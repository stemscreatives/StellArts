from datetime import datetime, timedelta

from fastapi import APIRouter, Depends, HTTPException, Query, status
from pydantic import BaseModel
from sqlalchemy.orm import Session

from app.core.auth import require_artisan
from app.core.calendar_crypto import encrypt_token
from app.core.config import settings
from app.db.session import get_db
from app.models.artisan import Artisan
from app.models.calendar import ArtisanCalendarConfig, ArtisanCalendarEvent
from app.models.user import User
from app.services.calendar_sync import calendar_sync_service

router = APIRouter(prefix="/calendar")


class CallbackPayload(BaseModel):
    code: str
    redirect_uri: str | None = None
    provider: str = "google"


@router.get("/auth-url")
def get_auth_url(
    provider: str = Query("google", pattern="^(google|outlook)$"),
    current_user: User = Depends(require_artisan),
):
    """Generate a read-only Google Calendar or Outlook OAuth URL for artisans."""
    redirect_uri = f"{settings.FRONTEND_URL}/calendar/callback"
    auth_url = calendar_sync_service.get_authorization_url(provider, redirect_uri)
    return {"auth_url": auth_url, "provider": provider}


@router.post("/callback")
async def oauth_callback(
    payload: CallbackPayload,
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Callback endpoint to handle Google authorization code exchange"""
    artisan = db.query(Artisan).filter(Artisan.user_id == current_user.id).first()
    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Artisan profile not found",
        )

    redirect_uri = payload.redirect_uri or f"{settings.FRONTEND_URL}/calendar/callback"

    provider = (
        payload.provider if payload.provider in {"google", "outlook"} else "google"
    )
    try:
        token_data = await calendar_sync_service.exchange_authorization_code(
            provider, payload.code, redirect_uri
        )
    except ValueError as e:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail=f"Failed to exchange {provider} OAuth code: {e}",
        ) from e
    except Exception as e:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail=f"Token exchange error: {str(e)}",
        ) from e

    access_token = token_data.get("access_token")
    refresh_token = token_data.get("refresh_token")
    expires_in = token_data.get("expires_in", 3600)
    enc_access = encrypt_token(access_token)
    enc_refresh = encrypt_token(refresh_token)
    expiry = datetime.utcnow() + timedelta(seconds=expires_in)

    config = (
        db.query(ArtisanCalendarConfig)
        .filter(ArtisanCalendarConfig.artisan_id == artisan.id)
        .first()
    )

    if config:
        config.provider = provider
        setattr(config, f"{provider}_access_token", enc_access)
        if refresh_token:
            setattr(config, f"{provider}_refresh_token", enc_refresh)
        config.token_expiry = expiry
    else:
        config = ArtisanCalendarConfig(
            artisan_id=artisan.id,
            provider=provider,
            **{
                f"{provider}_access_token": enc_access,
                f"{provider}_refresh_token": enc_refresh,
            },
            token_expiry=expiry,
            calendar_id="primary",
        )
        db.add(config)

    db.commit()

    # Trigger initial ingestion of events
    await calendar_sync_service.sync_artisan_calendar(db, artisan.id)

    return {
        "status": "success",
        "provider": provider,
        "message": f"{provider.title()} calendar successfully connected",
    }


@router.get("/status")
def get_sync_status(
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Get the Google Calendar connection status for the authenticated artisan"""
    artisan = db.query(Artisan).filter(Artisan.user_id == current_user.id).first()
    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Artisan profile not found",
        )

    config = (
        db.query(ArtisanCalendarConfig)
        .filter(ArtisanCalendarConfig.artisan_id == artisan.id)
        .first()
    )

    if not config:
        return {"connected": False, "last_synced_at": None}

    return {
        "connected": True,
        "calendar_id": config.calendar_id,
        "provider": config.provider,
        "last_synced_at": config.last_synced_at,
    }


@router.post("/disconnect")
def disconnect_calendar(
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Disconnect the Google Calendar and delete all local configurations and events"""
    artisan = db.query(Artisan).filter(Artisan.user_id == current_user.id).first()
    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Artisan profile not found",
        )

    config = (
        db.query(ArtisanCalendarConfig)
        .filter(ArtisanCalendarConfig.artisan_id == artisan.id)
        .first()
    )

    if config:
        db.delete(config)
        db.query(ArtisanCalendarEvent).filter(
            ArtisanCalendarEvent.artisan_id == artisan.id
        ).delete(synchronize_session=False)
        db.commit()

    return {"status": "success", "message": "Calendar disconnected"}


@router.post("/sync")
async def trigger_sync(
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Manually trigger ingestion of calendar events"""
    artisan = db.query(Artisan).filter(Artisan.user_id == current_user.id).first()
    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Artisan profile not found",
        )

    success = await calendar_sync_service.sync_artisan_calendar(db, artisan.id)
    if not success:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Failed to sync calendar events",
        )

    return {"status": "success", "message": "Calendar events successfully synced"}
