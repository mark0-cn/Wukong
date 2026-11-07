#include <stdio.h>
#include <string.h>

enum State { IDLE, OPEN, DIRTY, DONE };

int main(void) {
    enum State state = IDLE;
    char command[128];
    setvbuf(stdout, NULL, _IOLBF, 0);
    puts("READY");
    while (fgets(command, sizeof(command), stdin)) {
        command[strcspn(command, "\r\n")] = '\0';
        if (strcmp(command, "QUIT") == 0) { puts("BYE"); return 0; }
        if (strcmp(command, "RESET") == 0) { state = IDLE; puts("OK IDLE"); }
        else if (strcmp(command, "OPEN") == 0) {
            if (state != IDLE) puts("ERR state");
            else { state = OPEN; puts("OK OPEN"); }
        } else if (strcmp(command, "DATA") == 0) {
            if (state != OPEN && state != DIRTY) puts("ERR state");
            else { state = DIRTY; puts("OK DIRTY"); }
        } else if (strcmp(command, "COMMIT") == 0) {
            if (state != DIRTY) puts("ERR state");
            else { state = DONE; puts("OK DONE"); }
        } else puts("ERR command");
    }
    return 0;
}
