#include <errno.h>
#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

uint32_t transform_word(uint32_t value) {
    uint32_t mixed = value * UINT32_C(1664525) + UINT32_C(1013904223);
    return ((mixed << 7) | (mixed >> 25)) ^ UINT32_C(0xa5a5a5a5);
}

int main(int argc, char **argv) {
    if (argc != 2 || argv[1][0] == '\0' || argv[1][0] == '-') return 2;
    char *end = NULL;
    errno = 0;
    unsigned long long value = strtoull(argv[1], &end, 10);
    if (errno || *end != '\0' || value > UINT32_MAX) return 2;
    printf("%08" PRIx32 "\n", transform_word((uint32_t)value));
    return 0;
}
