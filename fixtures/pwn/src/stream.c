#define _GNU_SOURCE
#include <stdio.h>
#include <string.h>
#include <unistd.h>

/* Deliberately supplied FILE-pointer control, not a root-cause exploit. */
int main(int argc, char **argv) {
#ifndef __GLIBC__
    (void)argc; (void)argv;
    return 77;
#else
    FILE *stream = tmpfile();
    char buffer[256], output[64] = {0};
    char marker[] = "stream-marker\n";
    if (!stream || setvbuf(stream, buffer, _IOFBF, sizeof buffer)) return 2;
    if (fwrite("x", 1, 1, stream) != 1) return 3;
    if (argc > 1 && !strcmp(argv[1], "controlled")) {
        stream->_IO_read_base = marker;
        stream->_IO_read_ptr = marker;
        stream->_IO_read_end = marker;
        stream->_IO_write_base = marker;
        stream->_IO_write_ptr = marker + strlen(marker);
        stream->_IO_write_end = marker + sizeof marker;
    }
    if (argc > 2 && !strcmp(argv[2], "flush") && fflush(stream)) return 4;
    ssize_t count = pread(fileno(stream), output, sizeof output, 0);
    if (count < 0) return 5;
    if (count && write(STDOUT_FILENO, output, (size_t)count) != count) return 6;
    /* fclose may flush later; the observation above is deliberately earlier. */
    return fclose(stream) ? 7 : 0;
#endif
}
