"""Pure bounded numeric input validation."""
def bounded_int(value, default, minimum, maximum):
    value = int(value if value is not None else default)
    if not minimum <= value <= maximum:
        raise ValueError("numeric parameter outside allowed range")
    return value

def parse_address(value):
    """Accept the controller's interchangeable integer spellings for module-relative
    addresses: JSON integers, plain decimal strings and 0x-hex strings."""
    if isinstance(value, bool) or isinstance(value, float):
        raise ValueError("address must be a nonnegative integer or 0x-hex string")
    if isinstance(value, int):
        parsed = value
    elif isinstance(value, str):
        text = value.strip().replace("_", "")
        if not text:
            raise ValueError("address must be a nonnegative integer or 0x-hex string")
        parsed = int(text, 16) if text.lower().startswith("0x") else int(text, 10)
    else:
        raise ValueError("address must be a nonnegative integer or 0x-hex string")
    if not 0 <= parsed < 2**64:
        raise ValueError("address outside the 64-bit module-relative range")
    return parsed

