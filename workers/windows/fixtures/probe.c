#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <aclapi.h>
#include <dbghelp.h>
#include <wchar.h>
#include <string.h>
volatile LONG fixture_value = 0x12345678;
__declspec(noinline) LONG fixture_step(LONG value) { return value ^ 0x1020304; }
__declspec(noinline) void fixture_debug_wait(void) { Sleep(120000); }
static int copy_input(HANDLE input) {
    BYTE buffer[4096]; DWORD count = 0, written = 0;
    for (;;) {
        if (!ReadFile(input, buffer, sizeof(buffer), &count, NULL)) {
            if (GetLastError() == ERROR_BROKEN_PIPE) break;
            return 50;
        }
        if (!count) break;
        if (!WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), buffer, count, &written, NULL) || written != count) return 51;
    }
    return 0;
}
/* The restricted AppContainer SID must have WRITE_DAC at creation too. Owner
 * rights alone need not satisfy the token's restricted access check. */
static BOOL create_owned_cleanup_directory(void) {
    HANDLE token = NULL;
    BYTE information[4096], user_information[4096], storage[1024]; DWORD needed = 0;
    if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) return FALSE;
    BOOL queried = GetTokenInformation(token, TokenAppContainerSid, information, sizeof(information), &needed);
    BOOL user_queried = GetTokenInformation(token, TokenUser, user_information, sizeof(user_information), &needed);
    CloseHandle(token);
    PSID sid = queried ? ((TOKEN_APPCONTAINER_INFORMATION *)information)->TokenAppContainer : NULL;
    PSID user = user_queried ? ((TOKEN_USER *)user_information)->User.Sid : NULL;
    if (!sid || !IsValidSid(sid) || !user || !IsValidSid(user)) return FALSE;
    PACL acl = (PACL)storage;
    SECURITY_DESCRIPTOR descriptor;
    if (!InitializeAcl(acl, sizeof(storage), ACL_REVISION) ||
        !AddAccessAllowedAceEx(acl, ACL_REVISION, CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE, FILE_ALL_ACCESS, user) ||
        !AddAccessAllowedAceEx(acl, ACL_REVISION, CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE, FILE_ALL_ACCESS, sid) ||
        !InitializeSecurityDescriptor(&descriptor, SECURITY_DESCRIPTOR_REVISION) ||
        !SetSecurityDescriptorDacl(&descriptor, TRUE, acl, FALSE) ||
        !SetSecurityDescriptorControl(&descriptor, SE_DACL_PROTECTED, SE_DACL_PROTECTED)) return FALSE;
    SECURITY_ATTRIBUTES attributes = {sizeof(attributes), &descriptor, FALSE};
    return CreateDirectoryW(L"deny-tree", &attributes);
}
int wmain(int argc, wchar_t **argv) {
    if (argc == 2 && wcscmp(argv[1], L"--debug-wait") == 0) {
        fixture_debug_wait();
        return 0;
    }
    if (argc == 2 && wcscmp(argv[1], L"--debug-once") == 0) {
        fixture_value = fixture_step(fixture_value);
        return 0;
    }
    if (argc == 2 && wcscmp(argv[1], L"--copy-stdin") == 0) return copy_input(GetStdHandle(STD_INPUT_HANDLE));
    if ((argc == 3 && wcscmp(argv[1], L"--launch-stdin") == 0) ||
        (argc == 4 && wcscmp(argv[1], L"--launch-argv") == 0)) {
        wchar_t module[32768];
        DWORD size = GetModuleFileNameW(NULL, module, 32768);
        if (!size || size >= 32768) return 40;
        wchar_t *separator = wcsrchr(module, L'\\');
        if (!separator) return 41;
        separator[1] = 0;
        if (wcscat_s(module, 32768, L"wukong-fixture.dll") != 0) return 42;
        HMODULE dependency = LoadLibraryW(module);
        if (!dependency) return 43;
        typedef DWORD (__cdecl *ValueFunction)(void);
        ValueFunction value = (ValueFunction)(void *)GetProcAddress(dependency, "wukong_fixture_value");
        if (!value || value() != 0x574b4758) return 44;
        HANDLE peer = CreateFileW(argv[2], GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
        if (peer == INVALID_HANDLE_VALUE) return 45;
        char marker[12]; DWORD count = 0;
        BOOL read = ReadFile(peer, marker, sizeof(marker), &count, NULL);
        CloseHandle(peer);
        if (!read || count != 12 || memcmp(marker, "wukong-peer\n", 12) != 0) return 46;
        HANDLE output = CreateFileW(L"created.bin", GENERIC_WRITE, 0, NULL, CREATE_NEW, 0, NULL);
        if (output == INVALID_HANDLE_VALUE) return 47;
        DWORD written = 0;
        BOOL saved = WriteFile(output, marker, count, &written, NULL);
        CloseHandle(output);
        if (!saved || written != count) return 48;
        HANDLE input = argc == 3 ? GetStdHandle(STD_INPUT_HANDLE) : CreateFileW(argv[3], GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
        if (input == INVALID_HANDLE_VALUE) return 49;
        int result = copy_input(input);
        if (argc == 4) CloseHandle(input);
        FreeLibrary(dependency);
        return result;
    }
    if (argc == 2 && wcscmp(argv[1], L"--cleanup-adversary") == 0) {
        if (!create_owned_cleanup_directory()) return 20;
        HANDLE directory = CreateFileW(L"deny-tree", READ_CONTROL | WRITE_DAC,
            FILE_SHARE_READ | FILE_SHARE_WRITE, NULL, OPEN_EXISTING, FILE_FLAG_BACKUP_SEMANTICS, NULL);
        if (directory == INVALID_HANDLE_VALUE) return 27;
        HANDLE file = CreateFileW(L"deny-tree\\readonly.bin", GENERIC_WRITE, 0, NULL, CREATE_NEW, 0, NULL);
        if (file == INVALID_HANDLE_VALUE) return 21;
        DWORD written = 0;
        BOOL saved = WriteFile(file, "owned", 5, &written, NULL);
        CloseHandle(file);
        if (!saved || written != 5 || !SetFileAttributesW(L"deny-tree\\readonly.bin", FILE_ATTRIBUTE_READONLY)) return 22;
        ACL acl;
        if (!InitializeAcl(&acl, sizeof(acl), ACL_REVISION) ||
            SetSecurityInfo(directory, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                NULL, NULL, &acl, NULL) != ERROR_SUCCESS) return 23;
        PSECURITY_DESCRIPTOR actual = NULL; PACL observed = NULL;
        if (GetSecurityInfo(directory, SE_FILE_OBJECT, DACL_SECURITY_INFORMATION, NULL, NULL, &observed, NULL, &actual) != ERROR_SUCCESS) return 25;
        SECURITY_DESCRIPTOR_CONTROL control = 0; DWORD revision = 0;
        BOOL restricted = observed && observed->AceCount == 0 &&
            GetSecurityDescriptorControl(actual, &control, &revision) && (control & SE_DACL_PROTECTED);
        LocalFree(actual);
        CloseHandle(directory);
        if (!restricted) return 24;
        const char message[] = "wukong-cleanup-adversary\n";
        return WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), message, (DWORD)(sizeof(message)-1), &written, NULL) && written == (DWORD)(sizeof(message)-1) ? 0 : 26;
    }
    if (argc == 2 && wcscmp(argv[1], L"--echo") == 0) {
        DWORD written = 0;
        const char message[] = "wukong-worker-selftest\n";
        return WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), message, (DWORD)(sizeof(message)-1), &written, NULL) && written == (DWORD)(sizeof(message)-1) ? 0 : 5;
    }
    if (argc == 4 && wcscmp(argv[1], L"--read-argument") == 0) {
        if (wcscmp(argv[2], argv[3]) != 0) return 30;
        const DWORD share = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;
        HANDLE denied = CreateFileW(argv[2], GENERIC_WRITE, share, NULL, OPEN_EXISTING, 0, NULL);
        if (denied != INVALID_HANDLE_VALUE) { CloseHandle(denied); return 31; }
        if (GetLastError() != ERROR_ACCESS_DENIED) return 32;
        denied = CreateFileW(argv[2], FILE_EXECUTE, share, NULL, OPEN_EXISTING, 0, NULL);
        if (denied != INVALID_HANDLE_VALUE) { CloseHandle(denied); return 33; }
        if (GetLastError() != ERROR_ACCESS_DENIED) return 34;
        HANDLE file = CreateFileW(argv[2], GENERIC_READ, share, NULL, OPEN_EXISTING, 0, NULL);
        if (file == INVALID_HANDLE_VALUE) return 35;
        BYTE buffer[4096]; DWORD count = 0, written = 0;
        for (;;) {
            if (!ReadFile(file, buffer, sizeof(buffer), &count, NULL)) { CloseHandle(file); return 36; }
            if (!count) break;
            if (!WriteFile(GetStdHandle(STD_OUTPUT_HANDLE), buffer, count, &written, NULL) || written != count) { CloseHandle(file); return 37; }
        }
        CloseHandle(file);
        return 0;
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
    if (argc == 2 && wcscmp(argv[1], L"--fault-ready") == 0) {
        HANDLE file = CreateFileW(L"fault-ready.marker", GENERIC_WRITE, 0, NULL, CREATE_NEW, 0, NULL);
        if (file == INVALID_HANDLE_VALUE) return 28;
        DWORD written = 0;
        BOOL ready = WriteFile(file, "wukong-owned-fault-ready\n", 25, &written, NULL);
        CloseHandle(file);
        if (!ready || written != 25) return 29;
    }
    for (;;) { fixture_value = fixture_step(fixture_value); Sleep(20); }
}
