"""
Sample Python module for testing code chunking.

This module contains various Python constructs to test AST parsing.
"""

import os
import sys
from dataclasses import dataclass
from typing import Any, Dict, List, Optional

# Constants
MAX_RETRIES = 3
DEFAULT_TIMEOUT = 30.0


@dataclass
class User:
    """Represents a user in the system."""

    id: str
    name: str
    email: Optional[str] = None

    def get_display_name(self) -> str:
        """Return the display name for the user."""
        return self.name or self.email or self.id


class BaseService:
    """Base class for all services."""

    def __init__(self, config: Dict[str, Any]):
        """Initialize the service with configuration."""
        self.config = config
        self._initialized = False

    def initialize(self) -> None:
        """Initialize the service."""
        self._initialized = True

    def _validate_config(self) -> bool:
        """Validate the configuration (private method)."""
        return bool(self.config)


class UserService(BaseService):
    """Service for managing users."""

    def __init__(self, config: Dict[str, Any]):
        """Initialize UserService."""
        super().__init__(config)
        self.users: Dict[str, User] = {}

    def create_user(self, id: str, name: str, email: Optional[str] = None) -> User:
        """
        Create a new user.

        Args:
            id: User ID
            name: User name
            email: Optional email address

        Returns:
            The created User object
        """
        user = User(id=id, name=name, email=email)
        self.users[id] = user
        self._on_user_created(user)
        return user

    def get_user(self, id: str) -> Optional[User]:
        """Get a user by ID."""
        return self.users.get(id)

    def delete_user(self, id: str) -> bool:
        """Delete a user by ID."""
        if id in self.users:
            del self.users[id]
            return True
        return False

    def _on_user_created(self, user: User) -> None:
        """Internal callback when user is created."""
        pass


def calculate_factorial(n: int) -> int:
    """Calculate factorial of n."""
    if n <= 1:
        return 1
    return n * calculate_factorial(n - 1)


async def fetch_data(url: str, timeout: float = DEFAULT_TIMEOUT) -> Dict[str, Any]:
    """
    Fetch data from a URL asynchronously.

    Args:
        url: The URL to fetch from
        timeout: Request timeout in seconds

    Returns:
        The fetched data as a dictionary
    """
    # Simulated async fetch
    return {"url": url, "status": "ok"}


def process_items(items: List[str], *, validate: bool = True) -> List[str]:
    """Process a list of items with optional validation."""
    if validate:
        items = [item for item in items if item]
    return [item.strip().lower() for item in items]


# Module-level function that calls other functions
def main():
    """Main entry point."""
    service = UserService({"env": "test"})
    service.initialize()

    user = service.create_user("1", "Test User", "test@example.com")
    print(f"Created user: {user.get_display_name()}")

    result = calculate_factorial(5)
    print(f"Factorial: {result}")


if __name__ == "__main__":
    main()
