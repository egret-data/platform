"""egret_marimo: query egret from a marimo data app, as the viewer."""

from .client import GatewayError, SessionExpired, connect, sql

__all__ = ["GatewayError", "SessionExpired", "connect", "sql"]
