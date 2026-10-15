from datetime import UTC, datetime, timedelta
from urllib.parse import quote

import aiohttp
from sqlalchemy.orm import Session

from app.core.calendar_crypto import decrypt_token, encrypt_token
from app.core.config import settings
from app.models.calendar import ArtisanCalendarConfig, ArtisanCalendarEvent


class CalendarSyncService:
    """Read-only OAuth calendar ingestion for Google Calendar and Outlook."""

    def get_authorization_url(self, provider: str, redirect_uri: str) -> str:
        provider = self._normalize_provider(provider)
        if provider == "outlook":
            client_id = settings.OUTLOOK_CLIENT_ID or "mock_outlook_client_id"
            tenant = settings.OUTLOOK_TENANT_ID or "common"
            scope = quote("offline_access Calendars.Read")
            return (
                f"https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"
                f"?client_id={client_id}&redirect_uri={quote(redirect_uri, safe='')}"
                f"&response_type=code&scope={scope}&prompt=consent"
            )

        client_id = settings.GOOGLE_CLIENT_ID or "mock_client_id"
        scope = quote("https://www.googleapis.com/auth/calendar.readonly")
        return (
            "https://accounts.google.com/o/oauth2/v2/auth"
            f"?client_id={client_id}&redirect_uri={quote(redirect_uri, safe='')}"
            f"&response_type=code&scope={scope}&access_type=offline&prompt=consent"
        )

    async def exchange_authorization_code(
        self, provider: str, code: str, redirect_uri: str
    ) -> dict:
        provider = self._normalize_provider(provider)
        if provider == "outlook":
            if not settings.OUTLOOK_CLIENT_ID or not settings.OUTLOOK_CLIENT_SECRET:
                return self._mock_token_payload("outlook")
            token_url = f"https://login.microsoftonline.com/{settings.OUTLOOK_TENANT_ID}/oauth2/v2.0/token"
            data = {
                "client_id": settings.OUTLOOK_CLIENT_ID,
                "client_secret": settings.OUTLOOK_CLIENT_SECRET,
                "code": code,
                "grant_type": "authorization_code",
                "redirect_uri": redirect_uri,
                "scope": "offline_access Calendars.Read",
            }
        else:
            if not settings.GOOGLE_CLIENT_ID or not settings.GOOGLE_CLIENT_SECRET:
                return self._mock_token_payload("google")
            token_url = "https://oauth2.googleapis.com/token"
            data = {
                "client_id": settings.GOOGLE_CLIENT_ID,
                "client_secret": settings.GOOGLE_CLIENT_SECRET,
                "code": code,
                "grant_type": "authorization_code",
                "redirect_uri": redirect_uri,
            }

        async with aiohttp.ClientSession() as session:
            async with session.post(token_url, data=data) as response:
                body = await response.json(content_type=None)
                if response.status != 200:
                    raise ValueError(str(body))
                return body

    async def get_valid_access_token(
        self, db: Session, config: ArtisanCalendarConfig
    ) -> str | None:
        """Get a valid provider access token. Refreshes if expired."""
        provider = self._normalize_provider(config.provider)
        access_attr = f"{provider}_access_token"
        refresh_attr = f"{provider}_refresh_token"
        now = datetime.now(UTC)

        encrypted_access = getattr(config, access_attr, None)
        if (
            encrypted_access
            and config.token_expiry
            and config.token_expiry.replace(tzinfo=UTC) > now + timedelta(minutes=5)
        ):
            return decrypt_token(encrypted_access)

        refresh_token = decrypt_token(getattr(config, refresh_attr, None))
        if not refresh_token:
            return None

        if self._is_mock_mode(provider):
            setattr(config, access_attr, encrypt_token(f"mock_{provider}_access_token"))
            config.token_expiry = datetime.utcnow() + timedelta(hours=1)
            db.commit()
            return f"mock_{provider}_access_token"

        if provider == "outlook":
            url = f"https://login.microsoftonline.com/{settings.OUTLOOK_TENANT_ID}/oauth2/v2.0/token"
            payload = {
                "client_id": settings.OUTLOOK_CLIENT_ID,
                "client_secret": settings.OUTLOOK_CLIENT_SECRET,
                "refresh_token": refresh_token,
                "grant_type": "refresh_token",
                "scope": "offline_access Calendars.Read",
            }
        else:
            url = "https://oauth2.googleapis.com/token"
            payload = {
                "client_id": settings.GOOGLE_CLIENT_ID,
                "client_secret": settings.GOOGLE_CLIENT_SECRET,
                "refresh_token": refresh_token,
                "grant_type": "refresh_token",
            }

        try:
            async with aiohttp.ClientSession() as session:
                async with session.post(url, data=payload) as response:
                    data = await response.json(content_type=None)
                    if response.status != 200:
                        return None
                    access_token = data.get("access_token")
                    setattr(config, access_attr, encrypt_token(access_token))
                    config.token_expiry = datetime.utcnow() + timedelta(
                        seconds=data.get("expires_in", 3600)
                    )
                    if data.get("refresh_token"):
                        setattr(
                            config, refresh_attr, encrypt_token(data["refresh_token"])
                        )
                    db.commit()
                    return access_token
        except Exception as e:
            print(f"Error refreshing {provider} token: {e}")
            return None

    async def sync_artisan_calendar(self, db: Session, artisan_id: int) -> bool:
        """Ingest provider calendar events for the artisan into our local database."""
        config = (
            db.query(ArtisanCalendarConfig).filter_by(artisan_id=artisan_id).first()
        )
        if not config:
            return False

        access_token = await self.get_valid_access_token(db, config)
        if not access_token:
            return False

        provider = self._normalize_provider(config.provider)
        if access_token.startswith("mock_"):
            self._create_mock_calendar_events(db, artisan_id, provider)
            config.last_synced_at = datetime.utcnow()
            db.commit()
            return True

        time_min = datetime.now(UTC).isoformat()
        time_max = (datetime.now(UTC) + timedelta(days=30)).isoformat()
        if provider == "outlook":
            url = "https://graph.microsoft.com/v1.0/me/calendarView"
            params = {
                "startDateTime": time_min,
                "endDateTime": time_max,
                "$orderby": "start/dateTime",
            }
        else:
            url = f"https://www.googleapis.com/calendar/v3/calendars/{config.calendar_id}/events"
            params = {
                "timeMin": time_min,
                "timeMax": time_max,
                "singleEvents": "true",
                "orderBy": "startTime",
            }

        try:
            async with aiohttp.ClientSession() as session:
                async with session.get(
                    url,
                    params=params,
                    headers={"Authorization": f"Bearer {access_token}"},
                ) as response:
                    data = await response.json(content_type=None)
                    if response.status != 200:
                        print(
                            f"Failed to fetch {provider} events: {response.status} - {data}"
                        )
                        return False
                    raw_events = (
                        data.get("value", [])
                        if provider == "outlook"
                        else data.get("items", [])
                    )
                    self._upsert_events(db, artisan_id, provider, raw_events)
                    config.last_synced_at = datetime.utcnow()
                    db.commit()
                    return True
        except Exception as e:
            print(f"Error fetching {provider} events: {e}")
            return False

    def _upsert_events(
        self, db: Session, artisan_id: int, provider: str, events: list[dict]
    ) -> None:
        existing = (
            db.query(ArtisanCalendarEvent)
            .filter(ArtisanCalendarEvent.artisan_id == artisan_id)
            .all()
        )
        existing_map = {
            e.external_event_id: e
            for e in existing
            if e.external_event_id.startswith(f"{provider}:")
        }
        fetched_ids = set()
        for item in events:
            parsed = self._parse_event(provider, item)
            if not parsed:
                continue
            external_id = f"{provider}:{parsed['id']}"
            fetched_ids.add(external_id)
            local_event = existing_map.get(external_id)
            if not local_event:
                local_event = ArtisanCalendarEvent(
                    artisan_id=artisan_id, external_event_id=external_id
                )
                db.add(local_event)
            local_event.summary = parsed["summary"]
            local_event.start_time = parsed["start_time"]
            local_event.end_time = parsed["end_time"]
            local_event.location = parsed["location"]
        for ext_id, local_event in existing_map.items():
            if ext_id not in fetched_ids:
                db.delete(local_event)

    def _parse_event(self, provider: str, item: dict) -> dict | None:
        event_id = item.get("id")
        if not event_id:
            return None
        if provider == "outlook":
            start_str = (item.get("start") or {}).get("dateTime")
            end_str = (item.get("end") or {}).get("dateTime")
            location = (item.get("location") or {}).get("displayName")
            summary = item.get("subject") or "Busy"
        else:
            start_str = (item.get("start") or {}).get("dateTime") or (
                item.get("start") or {}
            ).get("date")
            end_str = (item.get("end") or {}).get("dateTime") or (
                item.get("end") or {}
            ).get("date")
            location = item.get("location")
            summary = item.get("summary") or "Busy"
        if not start_str or not end_str:
            return None
        return {
            "id": event_id,
            "summary": summary,
            "start_time": datetime.fromisoformat(
                start_str.replace("Z", "+00:00")
            ).replace(tzinfo=UTC),
            "end_time": datetime.fromisoformat(end_str.replace("Z", "+00:00")).replace(
                tzinfo=UTC
            ),
            "location": location,
        }

    def _create_mock_calendar_events(
        self, db: Session, artisan_id: int, provider: str = "google"
    ):
        db.query(ArtisanCalendarEvent).filter(
            ArtisanCalendarEvent.artisan_id == artisan_id,
            ArtisanCalendarEvent.external_event_id.like(f"{provider}:mock_event_%"),
        ).delete(synchronize_session=False)
        tomorrow = datetime.now(UTC).date() + timedelta(days=1)
        mock_events = [
            (
                "1",
                "Dentist Appointment",
                "09:00:00",
                "10:30:00",
                "123 Dental Clinic, NYC",
            ),
            ("2", "Lunch with Client", "12:00:00", "13:30:00", "456 Art Ave, Brooklyn"),
        ]
        for idx, summary, start, end, location in mock_events:
            db.add(
                ArtisanCalendarEvent(
                    artisan_id=artisan_id,
                    external_event_id=f"{provider}:mock_event_{idx}",
                    summary=summary,
                    start_time=datetime.combine(
                        tomorrow, datetime.strptime(start, "%H:%M:%S").time()
                    ).replace(tzinfo=UTC),
                    end_time=datetime.combine(
                        tomorrow, datetime.strptime(end, "%H:%M:%S").time()
                    ).replace(tzinfo=UTC),
                    location=location,
                )
            )
        db.commit()

    def _normalize_provider(self, provider: str | None) -> str:
        if provider and provider.lower() in {"google", "outlook"}:
            return provider.lower()
        return "google"

    def _mock_token_payload(self, provider: str) -> dict:
        return {
            "access_token": f"mock_{provider}_access_token",
            "refresh_token": f"mock_{provider}_refresh_token",
            "expires_in": 3600,
        }

    def _is_mock_mode(self, provider: str) -> bool:
        if provider == "outlook":
            return not settings.OUTLOOK_CLIENT_ID or not settings.OUTLOOK_CLIENT_SECRET
        return not settings.GOOGLE_CLIENT_ID or not settings.GOOGLE_CLIENT_SECRET


calendar_sync_service = CalendarSyncService()
