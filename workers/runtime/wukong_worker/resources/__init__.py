"""Durable job/session ownership and effect reconciliation."""

TERMINAL = {"completed", "failed", "cancelled", "timed_out", "lost"}
