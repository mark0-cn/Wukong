/* Negative control: bounded input and checked integer parsing. */
#include <stdio.h>
#include <stdlib.h>
#include <errno.h>
#include <limits.h>

int main(int argc, char **argv) {
    if (argc != 2) return 2;
    FILE *file = fopen(argv[1], "rb");
    if (!file) return 2;
    char text[64] = {0};
    size_t n = fread(text, 1, sizeof(text) - 1, file);
    int extra = fgetc(file), failed = ferror(file);
    fclose(file);
    if (failed || extra != EOF || n == 0) return 2;
    errno = 0;
    char *end = NULL;
    long value = strtol(text, &end, 10);
    if (errno || end == text || (*end != '\0' && *end != '\n') || value < INT_MIN || value > INT_MAX) return 2;
    if (*end == '\n' && end[1] != '\0') return 2;
    printf("value=%ld\n", value);
    return 0;
}
