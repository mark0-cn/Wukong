#define WIN32_LEAN_AND_MEAN
#include <windows.h>
DWORD __cdecl wukong_fixture_value(void) { return 0x574b4758; }
BOOL WINAPI DllMain(HINSTANCE instance, DWORD reason, LPVOID reserved) {
    UNREFERENCED_PARAMETER(instance);
    UNREFERENCED_PARAMETER(reason);
    UNREFERENCED_PARAMETER(reserved);
    return TRUE;
}
