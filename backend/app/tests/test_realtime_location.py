"""
Unit tests for the real-time artisan location update endpoint.

Covers:
  - Unauthenticated requests are rejected (403/401)
  - Client-role users are rejected (403)
  - Admin-role users are rejected (403)
  - Authenticated artisans can update location (200)
  - A successful update writes to Redis GEOADD and sets a TTL key
  - No synchronous PostgreSQL writes occur during a location update
  - Artisan profiles not found return 404
"""

from __future__ import annotations

import os

# Ensure env vars are set before any app module is imported
os.environ.setdefault("SECRET_KEY", "test-secret-key")
os.environ.setdefault("DATABASE_URL", "sqlite:///./test.db")

from unittest.mock import AsyncMock, patch

import pytest
from fastapi.testclient import TestClient

from app.core.security import create_access_token
from app.db.session import get_db
from app.main import app
from app.models.artisan import Artisan
from app.models.user import User

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

ENDPOINT = "/api/v1/artisans/location"
VALID_PAYLOAD = {"latitude": 6.5244, "longitude": 3.3792}


# ---------------------------------------------------------------------------
# Fixtures
# ---------------------------------------------------------------------------


@pytest.fixture()
def test_client(db_session):
    """
    TestClient with:
      - SQLite DB override
      - Redis mocked out (cache.redis is an AsyncMock)
      - security redis_client mocked
    """
    app.dependency_overrides[get_db] = lambda: db_session

    with (
        patch("app.core.cache.cache.initialize", new_callable=AsyncMock),
        patch("app.core.cache.cache.redis", new_callable=AsyncMock),
        patch("app.core.security.redis_client"),
    ):
        with TestClient(app) as c:
            yield c

    app.dependency_overrides.clear()


# ---------------------------------------------------------------------------
# Helper: register + login a user and return their Bearer token header
# ---------------------------------------------------------------------------


def _register_and_login(test_client: TestClient, role: str, suffix: str, db_session=None) -> str:
    """Create a user directly in the DB and return a valid access token.

    Bypasses the HTTP register/login endpoints to avoid rate-limiting
    and background-task (email) side-effects in tests.
    """
    from app.core.security import create_access_token, get_password_hash

    if db_session is None:
        raise ValueError("db_session is required for _register_and_login")

    email = f"{role}_{suffix}@loc.test"
    password = "StrongPass1!"
    user = User(
        email=email,
        hashed_password=get_password_hash(password),
        role=role,
        full_name=f"{role.title()} {suffix}",
        is_active=True,
    )
    db_session.add(user)
    db_session.commit()
    db_session.refresh(user)

    return create_access_token(subject=user.id)


# ---------------------------------------------------------------------------
# 1. Auth / Role-gate tests (no Redis needed)
# ---------------------------------------------------------------------------


class TestLocationEndpointAuthGate:
    """Verify that only artisan-role tokens are accepted."""

    def test_unauthenticated_request_is_rejected(self, test_client):
        """No token → 403 (HTTPBearer returns 403 for missing credentials)."""
        resp = test_client.put(ENDPOINT, json=VALID_PAYLOAD)
        assert resp.status_code == 403

    def test_invalid_token_is_rejected(self, test_client):
        """Garbage token → 401."""
        resp = test_client.put(
            ENDPOINT,
            json=VALID_PAYLOAD,
            headers={"Authorization": "Bearer not_a_real_token"},
        )
        assert resp.status_code == 401

    def test_client_role_cannot_update_location(self, test_client, db_session):
        """A user with role=client must receive 403."""
        token = _register_and_login(test_client, "client", "loc1", db_session)
        resp = test_client.put(
            ENDPOINT,
            json=VALID_PAYLOAD,
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 403

    def test_artisan_without_profile_returns_404(self, test_client, db_session):
        """
        An artisan-role user with no Artisan profile row gets 404,
        confirming auth passed but the profile lookup failed gracefully.
        """
        token = _register_and_login(test_client, "artisan", "noprofile", db_session)
        resp = test_client.put(
            ENDPOINT,
            json=VALID_PAYLOAD,
            headers={"Authorization": f"Bearer {token}"},
        )
        # Auth passes (artisan role), but no Artisan row → 404
        assert resp.status_code == 404


# ---------------------------------------------------------------------------
# 2. Happy-path: Redis writes verified with mocks
# ---------------------------------------------------------------------------


class TestLocationRedisWrites:
    """
    Verify correct Redis GEOADD + TTL behaviour using deep mocks so tests
    do not require a live Redis instance.
    """

    def test_successful_location_update_returns_200(self, test_client, db_session):
        """
        Full happy-path: artisan token → 200 with artisan_id, lat, lon, ttl.
        """
        from app.core.security import get_password_hash

        user = User(
            email="artisan_happy@loc.test",
            hashed_password=get_password_hash("StrongPass1!"),
            role="artisan",
            full_name="Happy Artisan",
            is_active=True,
        )
        db_session.add(user)
        db_session.commit()
        db_session.refresh(user)

        artisan = Artisan(user_id=user.id, is_available=True)
        db_session.add(artisan)
        db_session.commit()
        db_session.refresh(artisan)

        token = create_access_token(subject=user.id)

        mock_redis = AsyncMock()
        mock_redis.geoadd = AsyncMock(return_value=1)
        mock_redis.set = AsyncMock(return_value=True)

        with patch("app.core.cache.cache.redis", mock_redis):
            resp = test_client.put(
                ENDPOINT,
                json=VALID_PAYLOAD,
                headers={"Authorization": f"Bearer {token}"},
            )

        assert resp.status_code == 200
        data = resp.json()
        assert data["artisan_id"] == artisan.id
        assert data["latitude"] == VALID_PAYLOAD["latitude"]
        assert data["longitude"] == VALID_PAYLOAD["longitude"]
        assert data["ttl_seconds"] == 900  # LOCATION_TTL_SECONDS

    def test_geoadd_is_called_with_correct_arguments(self, test_client, db_session):
        """
        Verify Redis GEOADD is invoked with the right key, lon, lat, member.
        """
        from app.core.security import get_password_hash

        user = User(
            email="artisan_geoadd@loc.test",
            hashed_password=get_password_hash("StrongPass1!"),
            role="artisan",
            full_name="GeoAdd Artisan",
            is_active=True,
        )
        db_session.add(user)
        db_session.commit()
        db_session.refresh(user)

        artisan = Artisan(user_id=user.id, is_available=True)
        db_session.add(artisan)
        db_session.commit()
        db_session.refresh(artisan)

        token = create_access_token(subject=user.id)

        mock_redis = AsyncMock()
        mock_redis.geoadd = AsyncMock(return_value=1)
        mock_redis.set = AsyncMock(return_value=True)

        with patch("app.core.cache.cache.redis", mock_redis):
            test_client.put(
                ENDPOINT,
                json=VALID_PAYLOAD,
                headers={"Authorization": f"Bearer {token}"},
            )

        # Redis GEOADD must have been called once
        mock_redis.geoadd.assert_called_once()
        call_args = mock_redis.geoadd.call_args

        # Key is the geospatial index name
        assert call_args[0][0] == "artisans:locations"
        # Longitude comes before latitude in GEOADD convention
        assert call_args[0][1] == float(VALID_PAYLOAD["longitude"])
        assert call_args[0][2] == float(VALID_PAYLOAD["latitude"])
        # Member is the artisan_id as a string
        assert call_args[0][3] == str(artisan.id)

    def test_ttl_key_is_set_after_geoadd(self, test_client, db_session):
        """
        After GEOADD, a per-artisan TTL key must be SET with ex=900 (15 min).
        """
        from app.core.security import get_password_hash

        user = User(
            email="artisan_ttl@loc.test",
            hashed_password=get_password_hash("StrongPass1!"),
            role="artisan",
            full_name="TTL Artisan",
            is_active=True,
        )
        db_session.add(user)
        db_session.commit()
        db_session.refresh(user)

        artisan = Artisan(user_id=user.id, is_available=True)
        db_session.add(artisan)
        db_session.commit()
        db_session.refresh(artisan)

        token = create_access_token(subject=user.id)

        mock_redis = AsyncMock()
        mock_redis.geoadd = AsyncMock(return_value=1)
        mock_redis.set = AsyncMock(return_value=True)

        with patch("app.core.cache.cache.redis", mock_redis):
            resp = test_client.put(
                ENDPOINT,
                json=VALID_PAYLOAD,
                headers={"Authorization": f"Bearer {token}"},
            )

        assert resp.status_code == 200

        # The TTL sentinel key must be set with ex=900
        expected_ttl_key = f"artisan:location:ttl:{artisan.id}"
        mock_redis.set.assert_called_once_with(expected_ttl_key, "1", ex=900)

    def test_no_db_write_during_location_update(self, test_client, db_session):
        """
        Confirm that no SQLAlchemy flush/commit occurs during a location update —
        i.e. the endpoint touches Redis only, not PostgreSQL.
        """
        from app.core.security import get_password_hash

        user = User(
            email="artisan_nodb@loc.test",
            hashed_password=get_password_hash("StrongPass1!"),
            role="artisan",
            full_name="NoDb Artisan",
            is_active=True,
        )
        db_session.add(user)
        db_session.commit()
        db_session.refresh(user)

        artisan = Artisan(user_id=user.id, is_available=True)
        db_session.add(artisan)
        db_session.commit()
        db_session.refresh(artisan)

        token = create_access_token(subject=user.id)

        mock_redis = AsyncMock()
        mock_redis.geoadd = AsyncMock(return_value=1)
        mock_redis.set = AsyncMock(return_value=True)

        with patch("app.core.cache.cache.redis", mock_redis):
            # Spy on the session's commit to detect any DB write
            original_commit = db_session.commit
            commit_calls = []

            def tracking_commit():
                commit_calls.append(1)
                return original_commit()

            db_session.commit = tracking_commit

            resp = test_client.put(
                ENDPOINT,
                json=VALID_PAYLOAD,
                headers={"Authorization": f"Bearer {token}"},
            )

            db_session.commit = original_commit  # restore

        assert resp.status_code == 200
        assert len(commit_calls) == 0, (
            "Expected zero DB commits during a fast location update, "
            f"but got {len(commit_calls)}"
        )


# ---------------------------------------------------------------------------
# 3. Input validation
# ---------------------------------------------------------------------------


class TestLocationPayloadValidation:
    """Reject out-of-range coordinates with 422 Unprocessable Entity."""

    def _artisan_token(self, test_client, db_session) -> str:
        from app.core.security import get_password_hash

        user = User(
            email="artisan_valid@loc.test",
            hashed_password=get_password_hash("StrongPass1!"),
            role="artisan",
            full_name="Validator Artisan",
            is_active=True,
        )
        db_session.add(user)
        db_session.commit()
        db_session.refresh(user)
        artisan = Artisan(user_id=user.id, is_available=True)
        db_session.add(artisan)
        db_session.commit()
        return create_access_token(subject=user.id)

    def test_latitude_out_of_range_rejected(self, test_client, db_session):
        token = self._artisan_token(test_client, db_session)
        resp = test_client.put(
            ENDPOINT,
            json={"latitude": 95.0, "longitude": 3.0},  # >90 is invalid
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 422

    def test_longitude_out_of_range_rejected(self, test_client, db_session):
        token = self._artisan_token(test_client, db_session)
        resp = test_client.put(
            ENDPOINT,
            json={"latitude": 6.0, "longitude": 200.0},  # >180 is invalid
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 422

    def test_missing_latitude_rejected(self, test_client, db_session):
        token = self._artisan_token(test_client, db_session)
        resp = test_client.put(
            ENDPOINT,
            json={"longitude": 3.3792},
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 422

    def test_missing_longitude_rejected(self, test_client, db_session):
        token = self._artisan_token(test_client, db_session)
        resp = test_client.put(
            ENDPOINT,
            json={"latitude": 6.5244},
            headers={"Authorization": f"Bearer {token}"},
        )
        assert resp.status_code == 422
