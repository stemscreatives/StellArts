from __future__ import annotations

from fastapi import Depends, HTTPException, status
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer
from jose import JWTError
from sqlalchemy.orm import Session

from app.core.security import decode_token, is_token_blacklisted
from app.db.session import get_db
from app.models.user import User
from app.schemas.user import RoleEnum

# HTTP Bearer scheme for JWT token extraction
bearer_scheme = HTTPBearer(auto_error=False)


class AuthenticationError(HTTPException):
    """Custom exception for authentication errors (401)"""

    def __init__(self, detail: str = "Could not validate credentials"):
        super().__init__(
            status_code=status.HTTP_401_UNAUTHORIZED,
            detail=detail,
            headers={"WWW-Authenticate": "Bearer"},
        )


class AuthorizationError(HTTPException):
    """Custom exception for authorization errors (403)"""

    def __init__(self, detail: str = "Insufficient permissions"):
        super().__init__(
            status_code=status.HTTP_403_FORBIDDEN,
            detail=detail,
        )


def get_current_user(
    credentials: HTTPAuthorizationCredentials | None = Depends(bearer_scheme),
    db: Session = Depends(get_db),
) -> User:
    """
    Extract and validate JWT token, return current user.
    Raises 401 for invalid/expired/blacklisted tokens.
    """
    if credentials is None:
        raise AuthorizationError("Not authenticated")

    try:
        token = credentials.credentials
        payload = decode_token(token)
    except JWTError:
        raise AuthenticationError("Invalid token format") from None

    if not payload:
        raise AuthenticationError("Invalid token")

    # Check if token is blacklisted
    jti = payload.get("jti")
    if jti and is_token_blacklisted(jti):
        raise AuthenticationError("Token has been revoked")

    # Get user from database
    user_id = payload.get("sub")
    if not user_id:
        raise AuthenticationError("Token missing user information")

    user = db.query(User).filter(User.id == user_id).first()
    if not user:
        raise AuthenticationError("User not found")

    if not user.is_active:
        raise AuthenticationError("User account is inactive")

    return user


def get_current_active_user(current_user: User = Depends(get_current_user)) -> User:
    """
    Get current user and ensure they are active.
    """
    if not current_user.is_active:
        raise AuthenticationError("User account is inactive")
    return current_user


def require_roles(allowed_roles: list[RoleEnum]):
    """
    Create a dependency that requires specific roles.
    Returns a function that can be used as a FastAPI dependency.
    """

    def role_checker(current_user: User = Depends(get_current_active_user)) -> User:
        user_role = current_user.role

        # Convert string roles to RoleEnum for comparison
        allowed_role_values = [role.value for role in allowed_roles]

        if user_role not in allowed_role_values:
            role_names = ", ".join(allowed_role_values)
            raise AuthorizationError(
                f"Insufficient permissions. Access denied. Required roles: {role_names}. "
                f"Your role: {user_role}"
            )

        return current_user

    return role_checker


# Convenience dependencies for common role requirements
require_client = require_roles([RoleEnum.client])
require_artisan = require_roles([RoleEnum.artisan])
require_admin = require_roles([RoleEnum.admin])

# Combined role dependencies
require_client_or_artisan = require_roles([RoleEnum.client, RoleEnum.artisan])
require_artisan_or_admin = require_roles([RoleEnum.artisan, RoleEnum.admin])
require_any_role = require_roles([RoleEnum.client, RoleEnum.artisan, RoleEnum.admin])


def require_admin_or_self(target_user_id: int):
    """
    Create a dependency that allows admin users or the user themselves to access a resource.
    """

    def admin_or_self_checker(
        current_user: User = Depends(get_current_active_user),
    ) -> User:
        if (
            current_user.role == RoleEnum.admin.value
            or current_user.id == target_user_id
        ):
            return current_user

        raise AuthorizationError(
            "Access denied. You can only access your own resources or need admin privileges."
        )

    return admin_or_self_checker


def require_resource_owner_or_admin(resource_user_id: int):
    """
    Create a dependency that allows resource owners or admin users to access a resource.
    """

    def owner_or_admin_checker(
        current_user: User = Depends(get_current_active_user),
    ) -> User:
        if (
            current_user.role == RoleEnum.admin.value
            or current_user.id == resource_user_id
        ):
            return current_user

        raise AuthorizationError(
            "Access denied. You can only access resources you own or need admin privileges."
        )

    return owner_or_admin_checker
