/* Deliberately vulnerable local benchmark. Never ship as an application. */
#include <stdio.h>
#include <stdint.h>
#include <string.h>

__attribute__((noinline)) static unsigned copy_payload(const unsigned char *payload, size_t n) {
    unsigned char buffer[32] = {0};
#ifdef FIXED
    if (n > sizeof(buffer)) n = sizeof(buffer);
#endif
    /* Ground truth: missing size check in the non-FIXED build, CWE-121. */
    memcpy(buffer, payload, n);
    unsigned sum = 0;
    for (size_t i = 0; i < sizeof(buffer); ++i) sum += buffer[i];
    return sum;
}

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *file = fopen(argv[1], "rb");
    if (!file) return 2;
    unsigned char payload[256];
    size_t n = fread(payload, 1, sizeof(payload), file);
    int failed = ferror(file);
    fclose(file);
    if (failed) return 2;
    printf("checksum=%u\n", copy_payload(payload, n));
    return 0;
}
