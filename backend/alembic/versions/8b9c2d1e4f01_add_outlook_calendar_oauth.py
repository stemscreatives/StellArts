"""add_outlook_calendar_oauth

Revision ID: 8b9c2d1e4f01
Revises: 45a54bec1cb4
Create Date: 2026-07-26 00:00:00.000000

"""
from alembic import op
import sqlalchemy as sa


revision = "8b9c2d1e4f01"
down_revision = "45a54bec1cb4"
branch_labels = None
depends_on = None


def upgrade() -> None:
    op.add_column(
        "artisan_calendar_configs",
        sa.Column("outlook_access_token", sa.String(length=1024), nullable=True),
    )
    op.add_column(
        "artisan_calendar_configs",
        sa.Column("outlook_refresh_token", sa.String(length=1024), nullable=True),
    )
    op.add_column(
        "artisan_calendar_configs",
        sa.Column(
            "provider", sa.String(length=50), nullable=False, server_default="google"
        ),
    )
    op.alter_column("artisan_calendar_configs", "provider", server_default=None)


def downgrade() -> None:
    op.drop_column("artisan_calendar_configs", "provider")
    op.drop_column("artisan_calendar_configs", "outlook_refresh_token")
    op.drop_column("artisan_calendar_configs", "outlook_access_token")
