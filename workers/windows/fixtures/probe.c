#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <dbghelp.h>
#include <wchar.h>
volatile LONG fixture_value = 0x12345678;
__declspec(noinline) LONG fixture_step(LONG value) { return value ^ 0x1020304; }
int wmain(int argc, wchar_t **argv) {
    if (argc == 2 && wcscmp(argv[1], L"--cleanup-adversary") == 0) {
        if (!CreateDirectoryW(L"deny-tree", NULL)) return 20;
        HANDLE file = CreateFileW(L"deny-tree\\readonly.bin", GENERIC_WRITE, 0, NULL, CREATE_NEW, 0, NULL);
        if (file == INVALID_HANDLE_VALUE) return 21;
        DWORD written = 0;
        BOOL saved = WriteFile(file, "owned", 5, &written, NULL);
        CloseHandle(file);
        if (!saved || written != 5 || !SetFileAttributesW(L"deny-tree\\readonly.bin", FILE_ATTRIBUTE_READONLY)) return 22;
        ACL acl;
        SECURITY_DESCRIPTOR descriptor;
        if (!InitializeAcl(&acl, sizeof(acl), ACL_REVISION) ||
            !InitializeSecurityDescriptor(&descriptor, SECURITY_DESCRIPTOR_REVISION) ||
            !SetSecurityDescriptorDacl(&descriptor, TRUE, &acl, FALSE) ||
            !SetFileSecurityW(L"deny-tree", DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, &descriptor)) return 23;
        BYTE actual[4096]; DWORD needed = 0;
        if (GetFileSecurityW(L"deny-tree", DACL_SECURITY_INFORMATION, actual, sizeof(actual), &needed)) {
            PACL observed = NULL; BOOL present = FALSE, defaulted = FALSE;
            SECURITY_DESCRIPTOR_CONTROL control = 0; DWORD revision = 0;
            if (!GetSecurityDescriptorDacl(actual, &present, &observed, &defaulted) ||
                !GetSecurityDescriptorControl(actual, &control, &revision) ||
                !present || !observed || observed->AceCount != 0 || !(control & SE_DACL_PROTECTED)) return 24;
        } else if (GetLastError() != ERROR_ACCESS_DENIED) return 25;
        const char message[] = "wukong-cleanup-adversary\n";
        return WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), message, (DWORD)(sizeof(message)-1), &written, NULL) && written == (DWORD)(sizeof(message)-1) ? 0 : 26;
    }
    if (argc == 2 && wcscmp(argv[1], L"--echo") == 0) {
        DWORD written = 0;
        const char message[] = "wukong-worker-selftest\n";
        return WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), message, (DWORD)(sizeof(message)-1), &written, NULL) && written == (DWORD)(sizeof(message)-1) ? 0 : 5;
    }
    if (argc == 3 && wcscmp(argv[1], L"--replay") == 0) {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
        HANDLE file = CreateFileW(argv[2], GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
        if (file == INVALID_HANDLE_VALUE) return 4;
        char input[5] = {0}; DWORD count = 0;
        BOOL read = ReadFile(file, input, sizeof(input), &count, NULL);
        CloseHandle(file);
        if (read && count == 5 && input[0]=='C' && input[1]=='R' && input[2]=='A' && input[3]=='S' && input[4]=='H')
            RaiseException(EXCEPTION_ACCESS_VIOLATION, EXCEPTION_NONCONTINUABLE, 0, NULL);
        return 0;
    }
    if (argc == 3 && wcscmp(argv[1], L"--dump") == 0) {
        HANDLE file = CreateFileW(argv[2], GENERIC_WRITE, 0, NULL, CREATE_NEW, FILE_ATTRIBUTE_NORMAL, NULL);
        if (file == INVALID_HANDLE_VALUE) return 2;
        BOOL result = MiniDumpWriteDump(GetCurrentProcess(), GetCurrentProcessId(), file,
            MiniDumpWithFullMemory, NULL, NULL, NULL);
        CloseHandle(file);
        return result ? 0 : 3;
    }
    for (;;) { fixture_value = fixture_step(fixture_value); Sleep(20); }
}
