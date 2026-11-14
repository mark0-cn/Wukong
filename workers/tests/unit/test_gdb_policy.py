"""Interpreter-bearing MI arguments must not inherit the debugger's identity."""
import json
import unittest

from worker_gdb_policy import permitted_command


class GdbPolicyTests(unittest.TestCase):
    def test_host_calls_and_encoded_spellings_are_rejected_in_every_expression_position(self):
        expressions = ['$_shell("id")', '$_eval("shell id")', 'system("id")',
                       '(int)$_shell("id")', '$pc++', 'main=1', 'main;quit',
                       'main,$pc', 'foo()','$_shell', '$pc\n-exec-run']
        for expression in expressions:
            encoded = json.dumps(expression)
            commands = [f'-data-evaluate-expression {encoded}',
                        f'-data-read-memory-bytes {encoded} 16',
                        f'-data-write-memory-bytes {encoded} ff',
                        f'-break-condition 1 {encoded}',
                        f'-break-insert {encoded}', f'-exec-until {encoded}',
                        f'-data-disassemble -s {encoded} -e 0x400100 -- 0']
            for command in commands:
                with self.subTest(command=command), self.assertRaises(ValueError):
                    permitted_command(command)
        for encoded in [r'"\044\137shell(\"id\")"', r'"\x24_shell(\"id\")"',
                        r'"\u0024_shell(\"id\")"',r'"$pc\u000a-exec-run"']:
            with self.subTest(encoded=encoded), self.assertRaises(ValueError):
                permitted_command('-data-evaluate-expression '+encoded)

    def test_simple_register_pointer_symbol_and_arithmetic_inspection_remain_available(self):
        for expression in ['$pc', '$rsp + 0x10', '&main', '*$rsp', 'header->length',
                           'main', 'fixture::value', 'buffer[2]', '0x401000', 'value == 3']:
            with self.subTest(expression=expression):
                result = permitted_command('-data-evaluate-expression '+json.dumps(expression))
                self.assertEqual(result, '-data-evaluate-expression '+json.dumps(expression))
        self.assertEqual(permitted_command('-data-read-memory-bytes &main 16'),
                         '-data-read-memory-bytes "&main" "16"')
        self.assertEqual(permitted_command('-stack-list-frames --thread 2 0 8'),
                         '-stack-list-frames "--thread" "2" "0" "8"')

    def test_mutation_controls_cannot_smuggle_unchecked_options(self):
        for command in ['-break-insert -c "$_shell(\"id\")" main',
                        '-exec-run --all --interpreter=console',
                        '-data-read-memory-bytes main 65537',
                        '-data-write-memory-bytes main f',
                        '-stack-list-frames --thread 1 --thread 2',
                        '-interpreter-exec console "shell id"',
                        '-data-disassemble -s main -e "$_shell(\"id\")" -- 0',
                        '-stack-list-locals 1"x"', '-data-evaluate-expression main\\x28']:
            with self.subTest(command=command), self.assertRaises(ValueError):
                permitted_command(command)

    def test_dump_readonly_operations_still_validate_address_expressions(self):
        for command in ['-exec-run', '-data-write-memory-bytes main ff',
                        '-data-evaluate-expression "$pc"']:
            with self.subTest(command=command), self.assertRaisesRegex(ValueError, 'read-only'):
                permitted_command(command, dump=True)
        with self.assertRaises(ValueError):
            permitted_command('-data-read-memory-bytes "$_shell(\"id\")" 4', dump=True)
        self.assertEqual(permitted_command('-stack-list-frames', dump=True), '-stack-list-frames')
