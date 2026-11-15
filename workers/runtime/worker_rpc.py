"""JSON-RPC dispatch and standard errors belong to jsonrpcserver."""
from jsonrpcserver import dispatch, Success, Error
from jsonschema import ValidationError
from worker_common import error_kind
from worker_contract import validate_request, validate_result


class Server:
    def __init__(self, runtime):
        self.runtime = runtime
        self.methods = {name: self.handler(name) for name in runtime.handlers}

    def handler(self, name):
        def invoke(**params):
            try:
                validate_request(name, params)
            except (ValidationError, ValueError) as error:
                return Error(-32602, str(error)[:2048], {"kind": "invalid_request"})
            try:
                result = self.runtime.dispatch(name, params)
                validate_result(name, result)
                return Success(result)
            except Exception as error:
                return Error(-32000, str(error)[:4096], {"kind": error_kind(error)})
        return invoke

    def call(self, request):
        return dispatch(request, methods=self.methods, max_batch_size=1)
