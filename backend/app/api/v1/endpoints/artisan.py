from __future__ import annotations

import csv
import io
import json
import os
import uuid

from fastapi import APIRouter, Depends, File, HTTPException, Query, UploadFile, status
from fastapi.responses import StreamingResponse
from sqlalchemy.orm import Session, joinedload

from app.core.auth import (
    get_current_active_user,
    require_admin,
    require_artisan,
)
from app.db.session import get_db
from app.models.artisan import Artisan
from app.models.booking import Booking
from app.models.portfolio import Portfolio
from app.models.user import User
from app.schemas.artisan import (
    ArtisanAvailabilityUpdate,
    ArtisanLocationUpdate,
    ArtisanOut,
    ArtisanProfileCreate,
    ArtisanProfileResponse,
    ArtisanProfileUpdate,
    FastLocationResponse,
    FastLocationUpdate,
    GeolocationRequest,
    GeolocationResponse,
    NearbyArtisansRequest,
    NearbyArtisansResponse,
    PaginatedArtisans,
    PortfolioCreate,
)
from app.services.artisan import ArtisanService
from app.services.artisan_service import find_nearby_artisans_cached
from app.services.geolocation import LOCATION_TTL_SECONDS, geolocation_service

router = APIRouter(prefix="/artisans")


@router.get("/counts")
def get_artisan_counts(db: Session = Depends(get_db)):
    """Return available artisan counts grouped by specialty for the landing page.

    Parses the JSON-encoded ``specialties`` column and returns a dict whose keys
    are lower-cased specialty names and whose values are the number of *available*
    artisans that list that specialty.  A ``total`` key gives the overall count of
    available artisans regardless of specialty.
    """
    artisans = (
        db.query(Artisan.specialties)
        .filter(Artisan.is_available == True)  # noqa: E712
        .all()
    )

    counts: dict[str, int] = {}
    for (raw,) in artisans:
        if not raw:
            continue
        try:
            specs = json.loads(raw)
            if isinstance(specs, list):
                for s in specs:
                    key = str(s).strip().lower()
                    counts[key] = counts.get(key, 0) + 1
            else:
                key = str(specs).strip().lower()
                counts[key] = counts.get(key, 0) + 1
        except Exception:
            continue

    counts["total"] = len(artisans)
    return counts


# ✅ GET-based nearby artisans search (from Discovery&Filtering)
@router.get("/nearby", response_model=PaginatedArtisans)
async def get_nearby_artisans(
    *,
    db: Session = Depends(get_db),
    lat: float = Query(..., description="Latitude of the client location"),
    lon: float = Query(..., description="Longitude of the client location"),
    radius_km: float = Query(
        25.0, ge=0, le=200, description="Search radius in kilometers"
    ),
    specialties: list[str] | None = Query(None, description="Filter by skills"),
    min_rating: float | None = Query(None, ge=0, le=5, description="Min rating"),
    max_price: float | None = Query(None, ge=0, description="Max hourly rate"),
    min_experience: int | None = Query(None, ge=0, description="Min experience"),
    available: bool | None = Query(None, description="Filter by availability"),
    page: int = Query(1, ge=1),
    page_size: int = Query(10, ge=1, le=100),
):
    """
    Discover artisans nearby with optional filters for skill, minimum rating, and availability.
    Results are paginated and sorted by distance (asc) then rating (desc).
    """
    request = NearbyArtisansRequest(
        latitude=lat,
        longitude=lon,
        radius_km=radius_km,
        specialties=specialties,
        min_rating=min_rating,
        max_price=max_price,
        min_experience=min_experience,
        is_available=available if available is not None else True,
        limit=page_size * page,  # Fetch enough for pagination
    )

    result = await find_nearby_artisans_cached(db, request)

    # Manual pagination since service returns all matches within limit
    all_items = result.get("artisans", [])
    start = (page - 1) * page_size
    end = start + page_size
    paginated_items = all_items[start:end]

    return {
        "items": paginated_items,
        "total": result.get("total_found", 0),
        "page": page,
        "page_size": page_size,
    }


# ✅ POST-based nearby artisans search (from main)
@router.post("/nearby", response_model=NearbyArtisansResponse)
async def find_nearby_artisans(
    request: NearbyArtisansRequest, db: Session = Depends(get_db)
):
    """Find nearby artisans - public endpoint"""
    result = await find_nearby_artisans_cached(db, request)
    return result


@router.get("/me", response_model=ArtisanOut)
def get_my_artisan_profile(
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Get current artisan profile"""
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="Artisan profile not found"
        )
    return artisan


@router.get("/me/export")
def export_my_work_history(
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Export completed jobs and earnings for the authenticated artisan."""
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="Artisan profile not found"
        )

    output = io.StringIO()
    writer = csv.writer(output)
    writer.writerow(["Date", "Job Title", "Client ID", "Amount Earned", "Rating"])

    for row in service.get_completed_jobs_export_rows(artisan.id):
        writer.writerow(
            [
                row["Date"],
                row["Job Title"],
                row["Client ID"],
                row["Amount Earned"],
                row["Rating"],
            ]
        )

    filename = f"artisan-work-history-{artisan.id}.csv"
    headers = {"Content-Disposition": f'attachment; filename="{filename}"'}
    return StreamingResponse(
        iter([output.getvalue()]),
        media_type="text/csv",
        headers=headers,
    )


# Other artisan-related endpoints from main
@router.post("/profile", response_model=ArtisanOut)
async def create_artisan_profile(
    profile_data: ArtisanProfileCreate,
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Create artisan profile - artisan only"""
    service = ArtisanService(db)
    existing = service.get_artisan_by_user_id(current_user.id)
    if existing:
        raise HTTPException(
            status_code=status.HTTP_400_BAD_REQUEST,
            detail="Artisan profile already exists",
        )
    artisan = await service.create_artisan_profile(current_user.id, profile_data)
    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to create artisan profile",
        )
    return artisan


@router.put("/profile", response_model=ArtisanOut)
async def update_artisan_profile(
    profile_data: ArtisanProfileUpdate,
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Update artisan profile - artisan only"""
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="Artisan profile not found"
        )
    updated_artisan = await service.update_artisan_profile(artisan.id, profile_data)
    if not updated_artisan:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to update artisan profile",
        )
    return updated_artisan


@router.put("/location", response_model=FastLocationResponse)
async def update_artisan_location(
    location_data: FastLocationUpdate,
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Fast location update - stores coordinates in Redis only (no PostgreSQL writes).

    Use this endpoint for high-frequency location updates from artisan mobile devices.
    Location expires after 15 minutes of inactivity.
    """
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)

    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="Artisan profile not found"
        )

    # Add location to Redis geospatial index with TTL
    success = await geolocation_service.add_artisan_location(
        artisan.id,
        location_data.latitude,
        location_data.longitude,
    )

    if not success:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to update location in Redis",
        )

    return FastLocationResponse(
        artisan_id=artisan.id,
        latitude=location_data.latitude,
        longitude=location_data.longitude,
        ttl_seconds=LOCATION_TTL_SECONDS,
    )


@router.put("/location/full", response_model=ArtisanOut)
async def update_artisan_location_full(
    location_data: ArtisanLocationUpdate,
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Update artisan location with optional geocoding - artisan only.

    This endpoint updates both Redis and PostgreSQL. For high-frequency updates,
    use PUT /location instead.
    """
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="Artisan profile not found"
        )
    if location_data.location and not (
        location_data.latitude and location_data.longitude
    ):
        updated_artisan = await service.geocode_and_update_location(
            artisan.id, location_data.location
        )
        if not updated_artisan:
            raise HTTPException(
                status_code=status.HTTP_400_BAD_REQUEST,
                detail="Failed to geocode address",
            )
        return updated_artisan
    profile_update = ArtisanProfileUpdate(
        location=location_data.location,
        latitude=location_data.latitude,
        longitude=location_data.longitude,
    )
    updated_artisan = await service.update_artisan_profile(artisan.id, profile_update)
    if not updated_artisan:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to update location",
        )
    return updated_artisan


@router.post("/geocode", response_model=GeolocationResponse)
async def geocode_address(
    request: GeolocationRequest, current_user: User = Depends(get_current_active_user)
):
    """Convert address to coordinates - authenticated users only"""
    geo_result = await geolocation_service.geocode_address(request.address)
    if not geo_result:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND,
            detail="Address not found or geocoding failed",
        )
    return geo_result


@router.put("/availability", response_model=ArtisanOut)
async def update_availability(
    availability_data: ArtisanAvailabilityUpdate,
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Update artisan availability - artisan only"""
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(
            status_code=status.HTTP_404_NOT_FOUND, detail="Artisan profile not found"
        )

    updated_artisan = await service.update_artisan_availability(
        artisan.id, availability_data.is_available
    )
    if not updated_artisan:
        raise HTTPException(
            status_code=status.HTTP_500_INTERNAL_SERVER_ERROR,
            detail="Failed to update availability",
        )

    return updated_artisan


@router.get("/my-portfolio")
def get_my_portfolio(
    db: Session = Depends(get_db), current_user: User = Depends(require_artisan)
):
    """Get current artisan's portfolio items."""

    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(status_code=404, detail="Artisan profile not found")

    items = (
        db.query(Portfolio)
        .filter(Portfolio.artisan_id == artisan.id)
        .order_by(Portfolio.created_at.desc())
        .all()
    )
    return {
        "artisan_id": artisan.id,
        "artisan_name": current_user.full_name,
        "portfolio_items": [
            {
                "id": p.id,
                "artisan_id": p.artisan_id,
                "title": p.title,
                "image": p.image,
                "created_at": p.created_at,
            }
            for p in items
        ],
    }


@router.post("/portfolio/add", status_code=201)
def add_portfolio_item(
    item_in: PortfolioCreate,
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Add a portfolio item (JSON body with title and image_url)."""
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(status_code=404, detail="Artisan profile not found")

    item = Portfolio(
        artisan_id=artisan.id, title=item_in.title, image=item_in.image_url
    )
    db.add(item)
    db.commit()
    db.refresh(item)
    return {
        "id": item.id,
        "artisan_id": item.artisan_id,
        "title": item.title,
        "image": item.image,
        "created_at": item.created_at,
    }


@router.post("/portfolio/upload", status_code=201)
def upload_portfolio_image(
    title: str = None,
    file: UploadFile = File(None),
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Upload a portfolio image file and create a portfolio item.

    The file is stored locally under /tmp/stellarts_uploads and the generated
    URL is returned.  In production this would be replaced by an S3/CDN upload.
    """
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(status_code=404, detail="Artisan profile not found")
    if file is None:
        raise HTTPException(status_code=422, detail="file is required")

    ext = os.path.splitext(file.filename or "upload.bin")[1] or ".bin"
    filename = f"{uuid.uuid4().hex}{ext}"
    upload_dir = "/tmp/stellarts_uploads"
    os.makedirs(upload_dir, exist_ok=True)
    dest = os.path.join(upload_dir, filename)
    with open(dest, "wb") as f:
        f.write(file.file.read())

    image_url = f"/uploads/{filename}"
    item = Portfolio(artisan_id=artisan.id, title=title, image=image_url)
    db.add(item)
    db.commit()
    db.refresh(item)
    return {
        "id": item.id,
        "artisan_id": item.artisan_id,
        "title": item.title,
        "image": item.image,
        "image_url": image_url,
        "created_at": item.created_at,
    }


@router.put("/portfolio/{item_id}")
def update_portfolio_item(
    item_id: int,
    title: str = None,
    image_url: str = None,
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Update a portfolio item owned by the current artisan."""
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(status_code=404, detail="Artisan profile not found")

    item = (
        db.query(Portfolio)
        .filter(Portfolio.id == item_id, Portfolio.artisan_id == artisan.id)
        .first()
    )
    if not item:
        raise HTTPException(status_code=404, detail="Portfolio item not found")

    if title is not None:
        item.title = title
    if image_url is not None:
        item.image = image_url
    db.commit()
    db.refresh(item)
    return {
        "id": item.id,
        "artisan_id": item.artisan_id,
        "title": item.title,
        "image": item.image,
        "created_at": item.created_at,
    }


@router.delete("/portfolio/{item_id}", status_code=204)
def delete_portfolio_item(
    item_id: int,
    db: Session = Depends(get_db),
    current_user: User = Depends(require_artisan),
):
    """Delete a portfolio item owned by the current artisan."""
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(status_code=404, detail="Artisan profile not found")

    item = (
        db.query(Portfolio)
        .filter(Portfolio.id == item_id, Portfolio.artisan_id == artisan.id)
        .first()
    )
    if not item:
        raise HTTPException(status_code=404, detail="Portfolio item not found")

    db.delete(item)
    db.commit()
    return None


@router.get("/my-bookings")
def get_artisan_bookings(
    db: Session = Depends(get_db), current_user: User = Depends(require_artisan)
):
    """Get bookings assigned to current artisan"""
    service = ArtisanService(db)
    artisan = service.get_artisan_by_user_id(current_user.id)
    if not artisan:
        raise HTTPException(status_code=404, detail="Artisan profile not found")

    bookings = db.query(Booking).filter(Booking.artisan_id == artisan.id).all()

    return {
        "message": f"Bookings for artisan {current_user.id}",
        "artisan_name": current_user.full_name,
        "bookings": bookings,
    }


@router.get("/", response_model=list[ArtisanOut])
def list_artisans(
    db: Session = Depends(get_db),
    skip: int = Query(0, ge=0),
    limit: int = Query(20, ge=1, le=100),
    specialties: list[str] | None = Query(None),
    min_rating: float | None = Query(None, ge=0, le=5),
    max_price: float | None = Query(None, ge=0),
    min_experience: int | None = Query(None, ge=0),
    is_available: bool | None = Query(None),
    has_location: bool | None = Query(None),
):
    """List all artisans with optional filters - public endpoint"""
    service = ArtisanService(db)
    artisans = service.list_artisans(
        skip=skip,
        limit=limit,
        specialties=specialties,
        min_rating=min_rating,
        max_price=max_price,
        min_experience=min_experience,
        is_available=is_available,
        has_location=has_location,
    )
    return artisans


@router.get("/{artisan_id}/profile", response_model=ArtisanProfileResponse)
def get_artisan_profile(artisan_id: int, db: Session = Depends(get_db)):
    """Get specific artisan profile - public endpoint"""
    # Fetch artisan with user data eagerly to avoid N+1
    artisan = (
        db.query(Artisan)
        .options(joinedload(Artisan.user))
        .filter(Artisan.id == artisan_id)
        .first()
    )

    if not artisan or not artisan.user:
        raise HTTPException(status_code=404, detail="Artisan not found")

    # Fetch top 5 most recent portfolio items
    portfolio_items = (
        db.query(Portfolio)
        .filter(Portfolio.artisan_id == artisan_id)
        .order_by(Portfolio.created_at.desc())
        .limit(5)
        .all()
    )

    # Process specialties JSON
    specialty_str = None
    if artisan.specialties:
        try:
            specs = json.loads(artisan.specialties)
            if isinstance(specs, list):
                # Take the first one as primary or join them
                specialty_str = specs[0] if specs else None
            else:
                specialty_str = str(specs)
        except Exception:
            # Fallback if text is not JSON
            specialty_str = artisan.specialties

    # Construct response
    return {
        "id": artisan.id,
        "name": artisan.user.full_name,
        "avatar": artisan.user.avatar,
        "specialty": specialty_str,
        "rate": artisan.hourly_rate,
        "bio": artisan.description,
        "portfolio": portfolio_items,
        "average_rating": artisan.rating,
        "location": artisan.location,
    }


@router.delete("/{artisan_id}")
def delete_artisan(
    artisan_id: int,
    db: Session = Depends(get_db),
    current_user: User = Depends(require_admin),
):
    """Delete artisan account - admin only"""
    artisan = db.query(Artisan).filter(Artisan.id == artisan_id).first()
    if not artisan:
        raise HTTPException(status_code=404, detail="Artisan not found")

    db.delete(artisan)
    db.commit()
    return {
        "message": f"Artisan {artisan_id} deleted by admin {current_user.id}",
        "deleted_by": current_user.full_name,
    }
