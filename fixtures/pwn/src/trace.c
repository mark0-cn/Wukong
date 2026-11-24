#include <stdio.h>
#include <stdint.h>
#include <string.h>

#define NOINLINE __attribute__((noinline))

NOINLINE static size_t leaf(const unsigned char *input, size_t length, unsigned char *output) {
#ifdef VARIANT
    const unsigned char key = 0x5a;
#else
    const unsigned char key = 0x55;
#endif
    for (size_t index = 0; index < length; ++index) output[index] = input[index] ^ key;
    return length;
}

NOINLINE static size_t middle(const unsigned char *input, size_t length, unsigned char *output) {
    size_t first = leaf(input, length, output);
#ifdef VARIANT
    size_t second = 0;
#else
    size_t second = leaf(input, length, output + length);
#endif
    return first + second;
}

NOINLINE static unsigned recursive(unsigned value) {
    if (value <= 1) return 1;
    return value * recursive(value - 1);
}

NOINLINE static unsigned bad_pointer(unsigned char *input, size_t length) {
    return (unsigned)input[0] + (unsigned)length;
}

static void print_bytes(const unsigned char *bytes, size_t length) {
    for (size_t index = 0; index < length; ++index) printf("%02x", bytes[index]);
    putchar('\n');
}

int main(int argc, char **argv) {
    if (argc != 3) return 2;
    const char *mode = argv[1];
    unsigned char input[16];
    unsigned char output[32] = {0};
    FILE *stream = fopen(argv[2], "rb");
    if (!stream) return 3;
    size_t length = fread(input, 1, sizeof input, stream);
    fclose(stream);
    if (length == 0) return 4;
    if (strcmp(mode, "basic") == 0) {
        printf("ret=%zu\n", leaf(input, length, output));
        print_bytes(output, length);
        return 0;
    }
    if (strcmp(mode, "nested") == 0) {
        printf("ret=%zu\n", middle(input, length, output));
        print_bytes(output, length * 2);
        return 0;
    }
    if (strcmp(mode, "recursive") == 0) {
        printf("ret=%u\n", recursive(4));
        return 0;
    }
    if (strcmp(mode, "loop") == 0) {
        for (unsigned index = 0; index < 32; ++index) leaf(input, length, output);
        puts("done");
        return 0;
    }
    if (strcmp(mode, "bad") == 0) {
        printf("ret=%u\n", bad_pointer((unsigned char *)1, length));
        return 0;
    }
    if (strcmp(mode, "hang") == 0) {
        for (;;) { }
    }
    return 5;
}
