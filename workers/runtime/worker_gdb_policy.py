"""Bounded MI argument grammar for a debugger that owns protected metadata.

An operation-name allowlist is insufficient: address and condition arguments can
invoke GDB host convenience functions. Decode supported MI strings, validate
each argument's purpose, and serialize it again before reaching the interpreter.
"""
import json
import re


READ_ONLY = frozenset({
    "stack-list-frames", "stack-list-arguments", "stack-list-locals",
    "stack-list-variables", "stack-info-frame", "thread-info",
    "data-read-memory-bytes", "data-list-register-names",
    "data-list-register-values", "data-disassemble", "symbol-info-functions",
    "symbol-info-variables",
})
_IDENTIFIER = r"[A-Za-z_][A-Za-z0-9_]*(?:::[A-Za-z_][A-Za-z0-9_]*)*"
_ATOM = rf"(?:0[xX][0-9a-fA-F]+|[0-9]+|\$[A-Za-z][A-Za-z0-9_]*|{_IDENTIFIER})(?:(?:\.|->){_IDENTIFIER}|\[[0-9]+\])*"
_TERM = rf"[+\-~!*&]{{0,4}}{_ATOM}"
_EXPRESSION = re.compile(rf"{_TERM}(?:\s*(?:==|!=|<=|>=|&&|\|\||<<|>>|[+\-*/%&|^<>])\s*{_TERM})*")


def _expression(value):
    # No calls, casts, strings, assignments, parentheses or convenience
    # functions. Removing whitespace also catches increment/decrement spellings.
    compact = re.sub(r"\s", "", value)
    return (len(value) <= 1024 and "++" not in compact and "--" not in compact
            and _EXPRESSION.fullmatch(value) is not None)


def _integer(value, maximum=2**32-1, minimum=0):
    return re.fullmatch(r"[0-9]{1,20}", value) is not None and minimum <= int(value) <= maximum


def _breakpoint(value):
    return re.fullmatch(r"[0-9]{1,10}(?:\.[0-9]{1,10})?", value) is not None


def _arguments(text):
    result = []
    decoder = json.JSONDecoder()
    while text:
        text = text.lstrip(" \t")
        if not text:
            break
        if text.startswith('"'):
            try:
                value, end = decoder.raw_decode(text)
            except ValueError as error:
                raise ValueError("GDB/MI strings require supported JSON escapes; octal/hex escapes are rejected") from error
        else:
            match = re.match(r'[^\s"\\]+', text)
            if match is None:
                raise ValueError("unsupported GDB/MI argument quoting")
            value, end = match.group(), match.end()
        if end < len(text) and text[end] not in " \t":
            raise ValueError("GDB/MI arguments require whitespace separators")
        if any(ord(char) < 32 or ord(char) == 127 or 0xD800 <= ord(char) <= 0xDFFF or char == "\\" for char in value):
            raise ValueError("GDB/MI argument contains a control character or unsupported escape")
        result.append(value)
        if len(result) > 128:
            raise ValueError("GDB/MI argument limit exceeded")
        text = text[end:]
    return result


def _valid(operation, args):
    if operation == "break-insert":
        # Conditional/command-list options are not implicit interpreter access.
        while args and args[0] in {"-t", "-h", "-f", "-d"}:
            args = args[1:]
        return len(args) == 1 and _expression(args[0])
    if operation in {"break-delete", "break-enable", "break-disable"}:
        return bool(args) and all(_breakpoint(arg) for arg in args)
    if operation == "break-condition":
        return len(args) in {1, 2} and _breakpoint(args[0]) and (len(args) == 1 or _expression(args[1]))
    if operation == "break-after":
        return len(args) == 2 and _breakpoint(args[0]) and _integer(args[1])
    if operation in {"break-list", "stack-info-frame", "exec-run", "exec-finish"}:
        return not args
    if operation in {"exec-continue", "exec-interrupt"}:
        return not args or args == ["--all"]
    if operation in {"exec-next", "exec-next-instruction", "exec-step", "exec-step-instruction"}:
        return not args or len(args) == 1 and _integer(args[0], 1000000, 1)
    if operation == "exec-until":
        return not args or len(args) == 1 and _expression(args[0])
    if operation == "stack-list-frames":
        return not args or len(args) == 2 and all(_integer(arg, 4095) for arg in args) and int(args[0]) <= int(args[1])
    if operation in {"stack-list-arguments", "stack-list-locals", "stack-list-variables"}:
        if not args or args[0] not in {"0", "1", "2", "--no-values", "--all-values", "--simple-values"}:
            return False
        rest = args[1:]
        return not rest or operation == "stack-list-arguments" and len(rest) == 2 and all(_integer(arg, 4095) for arg in rest) and int(rest[0]) <= int(rest[1])
    if operation in {"stack-select-frame", "thread-select"}:
        return len(args) == 1 and _integer(args[0])
    if operation == "thread-info":
        return not args or len(args) == 1 and _integer(args[0])
    if operation == "data-evaluate-expression":
        return len(args) == 1 and _expression(args[0])
    if operation == "data-read-memory-bytes":
        return len(args) == 2 and _expression(args[0]) and _integer(args[1], 65536, 1)
    if operation == "data-write-memory-bytes":
        return (len(args) in {2, 3} and _expression(args[0])
                and re.fullmatch(r"(?:[0-9a-fA-F]{2}){1,65536}", args[1]) is not None
                and (len(args) == 2 or _integer(args[2], 65536, 1)))
    if operation == "data-list-register-names":
        return all(_integer(arg, 4095) for arg in args)
    if operation == "data-list-register-values":
        return bool(args) and args[0] in {"x", "d", "N", "t", "o", "r"} and all(_integer(arg, 4095) for arg in args[1:])
    if operation == "data-disassemble":
        return (len(args) == 6 and args[0] == "-s" and _expression(args[1])
                and args[2] == "-e" and _expression(args[3]) and args[4] == "--"
                and args[5] in {"0", "1", "2", "3", "4", "5"})
    if operation in {"symbol-info-functions", "symbol-info-variables"}:
        while args:
            if args[0] == "--include-nondebug":
                args = args[1:]
            elif len(args) >= 2 and args[0] in {"--name", "--type", "--max-results"}:
                if args[0] == "--max-results" and not _integer(args[1], 256, 1):
                    return False
                if len(args[1]) > 512:
                    return False
                args = args[2:]
            else:
                return False
        return True
    return False


def permitted_command(command, dump=False):
    match = re.fullmatch(r"-([a-z][a-z0-9-]*)(?: ([^\r\n\0]*))?", command)
    if match is None or len(command) > 32768:
        raise ValueError("one bounded GDB/MI command is required")
    operation = match[1]
    if dump and operation not in READ_ONLY:
        raise ValueError("dump sessions permit read-only GDB/MI inspection")
    args = _arguments(match[2] or "")
    context = []
    seen = set()
    while args and args[0] in {"--thread", "--frame"}:
        if len(args) < 2 or args[0] in seen or not _integer(args[1]):
            raise ValueError("invalid GDB/MI thread/frame context")
        seen.add(args[0])
        context.extend(args[:2])
        args = args[2:]
    if not _valid(operation, args):
        raise ValueError("GDB/MI arguments are not permitted; use typed inspection/control or simple noncalling expressions")
    return "-" + operation + "".join(" " + json.dumps(arg, ensure_ascii=False) for arg in context + args)
