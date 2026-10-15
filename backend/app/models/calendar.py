from sqlalchemy import Column, DateTime, ForeignKey, Integer, String
from sqlalchemy.orm import backref, relationship
from sqlalchemy.sql import func

from app.db.base import Base


class ArtisanCalendarConfig(Base):
    __tablename__ = "artisan_calendar_configs"

    id = Column(Integer, primary_key=True, index=True)
    artisan_id = Column(
        Integer,
        ForeignKey("artisans.id", ondelete="CASCADE"),
        unique=True,
        nullable=False,
    )
    google_access_token = Column(String(1024), nullable=True)  # Symmetrically encrypted
    google_refresh_token = Column(
        String(1024), nullable=True
    )  # Symmetrically encrypted
    outlook_access_token = Column(
        String(1024), nullable=True
    )  # Symmetrically encrypted
    outlook_refresh_token = Column(
        String(1024), nullable=True
    )  # Symmetrically encrypted
    provider = Column(String(50), default="google", nullable=False)
    token_expiry = Column(DateTime(timezone=True), nullable=True)
    calendar_id = Column(String(255), default="primary", nullable=False)
    last_synced_at = Column(DateTime(timezone=True), nullable=True)
    created_at = Column(DateTime(timezone=True), server_default=func.now())
    updated_at = Column(
        DateTime(timezone=True), server_default=func.now(), onupdate=func.now()
    )

    artisan = relationship(
        "Artisan",
        backref=backref("calendar_config", uselist=False, cascade="all, delete-orphan"),
    )


class ArtisanCalendarEvent(Base):
    __tablename__ = "artisan_calendar_events"

    id = Column(Integer, primary_key=True, index=True)
    artisan_id = Column(
        Integer, ForeignKey("artisans.id", ondelete="CASCADE"), nullable=False
    )
    external_event_id = Column(String(255), unique=True, index=True, nullable=False)
    summary = Column(String(500), nullable=True)
    start_time = Column(DateTime(timezone=True), nullable=False, index=True)
    end_time = Column(DateTime(timezone=True), nullable=False, index=True)
    location = Column(String(500), nullable=True)
    created_at = Column(DateTime(timezone=True), server_default=func.now())
    updated_at = Column(
        DateTime(timezone=True), server_default=func.now(), onupdate=func.now()
    )

    artisan = relationship(
        "Artisan", backref=backref("calendar_events", cascade="all, delete-orphan")
    )
