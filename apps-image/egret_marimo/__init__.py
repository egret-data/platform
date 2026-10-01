"""egret_marimo: query egret from a marimo data app, as the viewer."""

from .client import Connection, GatewayError, SessionExpired, connect, sql

__all__ = ["Connection", "GatewayError", "SessionExpired", "connect", "sql"]
