#include <stdio.h>
#include <string.h>

/* Original toy format: literal tag+1 bytes, or back-reference (tag&127)+3. */
int main(void) {
    struct { unsigned char output[16]; unsigned char protected[8]; } state;
    memset(&state, 0, sizeof state);
    memset(state.protected, '.', sizeof state.protected);
    size_t used = 0;
    int tag;
    while ((tag = getchar()) != EOF) {
        size_t length = tag < 128 ? (size_t)tag + 1 : (size_t)(tag & 127) + 3;
#ifdef FIXED
        if (length > sizeof state.output - used) { puts("bounds-denied"); return 3; }
#endif
        /* Bound the trusted regression even in its deliberately vulnerable form. */
        if (used + length > sizeof state) { puts("fixture-budget"); return 4; }
        if (tag < 128) {
            for (size_t i = 0; i < length; ++i) {
                int byte = getchar();
                if (byte == EOF) { puts("truncated"); return 5; }
                state.output[used++] = (unsigned char)byte;
            }
        } else {
            int distance = getchar();
            if (distance <= 0 || (size_t)distance > used) { puts("distance-denied"); return 6; }
            for (size_t i = 0; i < length; ++i) {
                state.output[used] = state.output[used - (size_t)distance];
                ++used;
            }
        }
    }
    if (!memcmp(state.protected, "PWND", 4)) puts("protected-changed");
    else printf("decoded=%zu\n", used);
    return 0;
}
