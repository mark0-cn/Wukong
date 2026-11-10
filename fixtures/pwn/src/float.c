#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

int main(void) {
    _Static_assert(sizeof(double) == sizeof(uint64_t), "binary64 fixture required");
    double value;
    uint64_t bits;
    if (scanf("%lf", &value) != 1) return 2;
    memcpy(&bits, &value, sizeof bits);
    printf("%016" PRIx64 "\n", bits);
    return 0;
}
