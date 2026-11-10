#include <stdio.h>
int main(void) {
    /* The small positional-format counterexample is intentional. */
    return printf("%2$d:%1$d\n", 11, 22) < 0;
}
