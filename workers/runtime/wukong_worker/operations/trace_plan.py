"""Validation and canonical logical plans for the public dynamic-tracing protocol."""

import copy
import hashlib
import json
import re

_SCHEMA_VERSION = 1
_ABI = "sysv_amd64"
_PLAN_DOMAIN = b"wukong.trace.plan.v1\0"
_INTEGER_TYPES = frozenset({"u8", "u16", "u32", "u64", "i8", "i16", "i32", "i64", "pointer"})
_LIMIT_DEFAULTS = {
    "max_events": 1024,
    "max_point_hits": 256,
    "max_buffer_bytes": 4096,
    "max_event_bytes": 65536,
    "max_events_bytes": 8 * 1024 * 1024,
    "stop_wait_ms": 10000,
}
_LIMIT_RANGES = {
    "max_events": (1, 4096),
    "max_point_hits": (1, 1024),
    "max_buffer_bytes": (1, 65536),
    "max_event_bytes": (1024, 262144),
    "max_events_bytes": (4096, 16 * 1024 * 1024),
    "stop_wait_ms": (100, 30000),
}
_IDENTIFIER = re.compile(r"[A-Za-z][A-Za-z0-9_.-]{0,63}\Z")
_MODULE_SHA256 = re.compile(r"[0-9a-f]{64}\Z")
_SYMBOL = re.compile(r"[A-Za-z_][A-Za-z0-9_]*\Z")


def _object(value, path, allowed):
    if not isinstance(value, dict):
        raise ValueError(f"{path} must be an object")
    unknown = set(value) - set(allowed)
    if unknown:
        raise ValueError(f"{path} has unknown fields: {sorted(unknown)!r}")
    return value


def _required(value, field, path):
    if field not in value:
        raise ValueError(f"{path}.{field} is required")
    return value[field]


def _list(value, path, minimum=None, maximum=None):
    if not isinstance(value, list):
        raise ValueError(f"{path} must be an array")
    if minimum is not None and len(value) < minimum:
        raise ValueError(f"{path} has too few entries")
    if maximum is not None and len(value) > maximum:
        raise ValueError(f"{path} has too many entries")
    return value


def _integer(value, path, minimum=0, maximum=(1 << 64) - 1):
    if type(value) is not int or not minimum <= value <= maximum:
        raise ValueError(f"{path} must be an integer in {minimum}..{maximum}")
    return value


def _string(value, path):
    if not isinstance(value, str):
        raise ValueError(f"{path} must be a string")
    return value


def _validate_identifier(value, path="point_id"):
    value = _string(value, path)
    if _IDENTIFIER.fullmatch(value) is None:
        raise ValueError(f"invalid {path}")
    return value


def _validate_name(value, path):
    value = _string(value, path)
    if not 1 <= len(value) <= 64:
        raise ValueError(f"{path} must contain 1..64 characters")
    return value


def _validate_module_sha256(value, path):
    value = _string(value, path)
    if _MODULE_SHA256.fullmatch(value) is None:
        raise ValueError(f"{path} must be 64 lowercase hexadecimal characters")
    return value


def _validate_symbol(value, path):
    value = _string(value, path)
    if _SYMBOL.fullmatch(value) is None:
        raise ValueError(f"{path} must be a safe plain symbol")
    return value


def _validate_index(value, path):
    return _integer(value, path, 0, 5)


def _normalize_location(value, path):
    value = _object(value, path, {"kind", "module_sha256", "symbol", "offset"})
    kind = _string(_required(value, "kind", path), f"{path}.kind")
    module_sha256 = _validate_module_sha256(
        _required(value, "module_sha256", path), f"{path}.module_sha256"
    )
    if kind == "symbol":
        _object(value, path, {"kind", "module_sha256", "symbol"})
        return {
            "kind": kind,
            "module_sha256": module_sha256,
            "symbol": _validate_symbol(_required(value, "symbol", path), f"{path}.symbol"),
        }
    if kind == "module_offset":
        _object(value, path, {"kind", "module_sha256", "offset"})
        return {
            "kind": kind,
            "module_sha256": module_sha256,
            "offset": _integer(_required(value, "offset", path), f"{path}.offset"),
        }
    raise ValueError(f"{path}.kind is unsupported")


def _normalize_length(value, path):
    value = _object(value, path, {"kind", "bytes", "index"})
    kind = _string(_required(value, "kind", path), f"{path}.kind")
    if kind == "constant":
        _object(value, path, {"kind", "bytes"})
        return {
            "kind": kind,
            "bytes": _integer(_required(value, "bytes", path), f"{path}.bytes", 0, 1024 * 1024),
        }
    if kind == "argument":
        _object(value, path, {"kind", "index"})
        return {
            "kind": kind,
            "index": _validate_index(_required(value, "index", path), f"{path}.index"),
        }
    raise ValueError(f"{path}.kind is unsupported")


def _normalize_capture(value, path, kind, phases):
    value = _object(value, path, {"arguments", "buffers", "return_value"})
    arguments = []
    buffers = []
    names = set()
    indices = set()
    for position, argument in enumerate(_list(value.get("arguments", []), f"{path}.arguments")):
        item_path = f"{path}.arguments[{position}]"
        argument = _object(argument, item_path, {"name", "index", "value_type"})
        name = _validate_name(_required(argument, "name", item_path), f"{item_path}.name")
        if name in names:
            raise ValueError(f"duplicate capture name {name!r} in {path}")
        names.add(name)
        index = _validate_index(_required(argument, "index", item_path), f"{item_path}.index")
        if index in indices:
            raise ValueError(f"duplicate argument index {index} in {path}")
        indices.add(index)
        value_type = _string(
            _required(argument, "value_type", item_path), f"{item_path}.value_type"
        )
        if value_type not in _INTEGER_TYPES:
            raise ValueError(f"{item_path}.value_type is unsupported")
        arguments.append({"name": name, "index": index, "value_type": value_type})
    for position, buffer in enumerate(_list(value.get("buffers", []), f"{path}.buffers")):
        item_path = f"{path}.buffers[{position}]"
        buffer = _object(buffer, item_path, {"name", "address_argument", "length"})
        name = _validate_name(_required(buffer, "name", item_path), f"{item_path}.name")
        if name in names:
            raise ValueError(f"duplicate capture name {name!r} in {path}")
        names.add(name)
        buffers.append(
            {
                "name": name,
                "address_argument": _validate_index(
                    _required(buffer, "address_argument", item_path),
                    f"{item_path}.address_argument",
                ),
                "length": _normalize_length(
                    _required(buffer, "length", item_path), f"{item_path}.length"
                ),
            }
        )
    normalized = {"arguments": arguments, "buffers": buffers}
    return_value = value.get("return_value")
    if return_value is not None:
        return_value = _string(return_value, f"{path}.return_value")
        if return_value not in _INTEGER_TYPES:
            raise ValueError(f"{path}.return_value is unsupported")
        if kind != "function" or "return" not in phases:
            raise ValueError(f"{path}.return_value requires a return function")
        normalized["return_value"] = return_value
    return normalized


def _normalize_point(value, path):
    value = _object(value, path, {"kind", "point_id", "location", "phases", "capture"})
    kind = _string(_required(value, "kind", path), f"{path}.kind")
    if kind not in {"function", "call_site"}:
        raise ValueError(f"{path}.kind is unsupported")
    point_id = _validate_identifier(_required(value, "point_id", path), f"{path}.point_id")
    normalized = {
        "kind": kind,
        "point_id": point_id,
        "location": _normalize_location(_required(value, "location", path), f"{path}.location"),
    }
    phases = []
    if kind == "function":
        _object(value, path, {"kind", "point_id", "location", "phases", "capture"})
        phases = _list(_required(value, "phases", path), f"{path}.phases", minimum=1, maximum=2)
        if any(not isinstance(phase, str) or phase not in {"entry", "return"} for phase in phases):
            raise ValueError(f"{path}.phases contains an invalid phase")
        if len(set(phases)) != len(phases):
            raise ValueError(f"{path}.phases contains a duplicate")
        normalized["phases"] = list(phases)
    else:
        _object(value, path, {"kind", "point_id", "location", "capture"})
    normalized["capture"] = _normalize_capture(
        _required(value, "capture", path), f"{path}.capture", kind, phases
    )
    return normalized


def _normalize_limits(value):
    value = _object(value, "limits", set(_LIMIT_DEFAULTS))
    normalized = {}
    for field, default in _LIMIT_DEFAULTS.items():
        normalized[field] = _integer(
            value.get(field, default), f"limits.{field}", *_LIMIT_RANGES[field]
        )
    if normalized["max_event_bytes"] > normalized["max_events_bytes"]:
        raise ValueError("limits.max_event_bytes exceeds limits.max_events_bytes")
    return normalized


def validate_spec(spec):
    """Return a defensive, default-filled copy or raise ValueError for invalid semantics."""
    spec = _object(spec, "spec", {"schema_version", "abi", "points", "limits"})
    schema_version = _integer(spec.get("schema_version", 1), "schema_version", 1, 1)
    abi = _string(spec.get("abi", _ABI), "abi")
    if abi != _ABI:
        raise ValueError("abi is unsupported")
    points = [
        _normalize_point(point, f"points[{position}]")
        for position, point in enumerate(
            _list(_required(spec, "points", "spec"), "points", minimum=1, maximum=64)
        )
    ]
    point_ids = [point["point_id"] for point in points]
    if len(set(point_ids)) != len(point_ids):
        raise ValueError("points contains a duplicate point_id")
    return {
        "schema_version": schema_version,
        "abi": abi,
        "points": points,
        "limits": _normalize_limits(spec.get("limits", {})),
    }


def _logical_capture(capture):
    normalized = {
        "arguments": sorted(capture["arguments"], key=lambda item: item["name"]),
        "buffers": sorted(capture["buffers"], key=lambda item: item["name"]),
    }
    if capture.get("return_value") is not None:
        normalized["return_value"] = capture["return_value"]
    return copy.deepcopy(normalized)


def logical_plan(spec):
    """Return canonical trace semantics, excluding locations and execution limits."""
    spec = validate_spec(spec)
    phase_order = {"entry": 0, "return": 1}
    points = []
    for original in sorted(spec["points"], key=lambda point: point["point_id"]):
        point = {
            "kind": original["kind"],
            "point_id": original["point_id"],
            "capture": _logical_capture(original["capture"]),
        }
        if original["kind"] == "function":
            point["phases"] = sorted(original["phases"], key=phase_order.__getitem__)
        points.append(point)
    return {"schema_version": spec["schema_version"], "abi": spec["abi"], "points": points}


def _canonical_json(spec):
    return json.dumps(
        logical_plan(spec),
        ensure_ascii=False,
        allow_nan=False,
        sort_keys=True,
        separators=(",", ":"),
    )


def plan_signature(spec):
    canonical = _canonical_json(spec).encode("utf-8")
    return hashlib.sha256(_PLAN_DOMAIN + canonical).hexdigest()
