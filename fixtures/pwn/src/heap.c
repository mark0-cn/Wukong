#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    char *original = malloc(48);
    if (!original) return 2;
    char *alias = original;
    if (argc > 1 && !strcmp(argv[1], "resize")) {
        char *result = realloc(original, 0);
#ifdef FIXED
        alias = NULL;
#endif
        printf("returned_null=%d stale_alias=%d\n", result == NULL, alias != NULL);
        free(result);
        return 0;
    }
    uintptr_t old_address = (uintptr_t)original;
    free(original);
#ifdef FIXED
    alias = NULL;
#endif
    char *current = malloc(48);
    if (!current) return 3;
    if ((uintptr_t)current != old_address) { free(current); return 77; }
    strcpy(current, "ordinary");
    if (argc > 1 && !strcmp(argv[1], "edit")) {
        if (!alias) { puts("denied"); free(current); return 0; }
        /* Intentional stale-alias defect, limited to the reused fixture object. */
        strcpy(alias, "changed");
    }
    puts(current);
    free(current);
    return 0;
}
