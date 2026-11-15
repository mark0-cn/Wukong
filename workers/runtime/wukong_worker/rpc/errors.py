"""Stable worker error categories used by RPC and durable effect receipts."""


class WorkerError(ValueError):
    def __init__(self, kind: str, message: str) -> None:
        super().__init__(message)
        self.kind = kind


def error_kind(error: Exception) -> str:
    return getattr(
        error,
        "kind",
        "invalid_request" if isinstance(error, (ValueError, KeyError, TypeError)) else "internal",
    )
