#include <stdlib.h>
#include <string.h>
#include <unistd.h>

static void normal(void) { (void)write(1, "atexit-marker\n", 14); }
static void quick(void) { (void)write(1, "quick-marker\n", 13); }

int main(int argc, char **argv) {
    if (atexit(normal) || at_quick_exit(quick)) return 2;
    if (argc > 1 && !strcmp(argv[1], "immediate")) _Exit(0);
    if (argc > 1 && !strcmp(argv[1], "quick")) quick_exit(0);
    if (argc > 1 && !strcmp(argv[1], "exit")) exit(0);
    return 0;
}
