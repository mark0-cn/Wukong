#include <stdio.h>
#include <string.h>

struct object { unsigned generation; int live; char data[16]; };
struct handle { unsigned index, generation; };

int main(int argc, char **argv) {
    struct object objects[1] = {{1, 1, "old"}};
    struct handle stale = {0, 1};
    if (argc > 1 && !strcmp(argv[1], "collect")) {
        objects[0].live = 0;
        ++objects[0].generation;
        objects[0].live = 1;
        strcpy(objects[0].data, "protected");
    }
    struct object *object = &objects[stale.index];
#ifdef FIXED
    if (!object->live || object->generation != stale.generation) { puts("stale-denied"); return 0; }
#endif
    strcpy(object->data, "edited");
    puts(object->data);
    return 0;
}
