#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#ifndef RECORD_VERSION
#define RECORD_VERSION 1
#endif

typedef struct {
    uint8_t version;
    uint8_t flags;
    uint32_t payload_length;
    const uint8_t *payload;
} Record;

int parse_record(const uint8_t *bytes, size_t size, Record *record) {
    const size_t header = RECORD_VERSION == 1 ? 8 : 10;
    if (size < header + 1) return 2;
    if (memcmp(bytes, "RECD", 4) != 0 || bytes[4] != RECORD_VERSION) return 3;
    uint32_t length = (uint32_t)bytes[6] | ((uint32_t)bytes[7] << 8);
    if (RECORD_VERSION == 2)
        length |= ((uint32_t)bytes[8] << 16) | ((uint32_t)bytes[9] << 24);
    if (length != size - header - 1) return 4;
    uint8_t checksum = 0;
    for (uint32_t i = 0; i < length; ++i) checksum ^= bytes[header + i];
    if (checksum != bytes[size - 1]) return 5;
    record->version = bytes[4];
    record->flags = bytes[5];
    record->payload_length = length;
    record->payload = bytes + header;
    return 0;
}

void describe_record(const Record *record) {
    printf("version=%u flags=%u length=%" PRIu32 "\n",
           record->version, record->flags, record->payload_length);
}

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    FILE *stream = fopen(argv[1], "rb");
    if (!stream) return 1;
    uint8_t bytes[4096];
    size_t size = fread(bytes, 1, sizeof(bytes), stream);
    int extra = fgetc(stream);
    int failed = ferror(stream);
    fclose(stream);
    if (failed || extra != EOF) return 6;
    Record record;
    int status = parse_record(bytes, size, &record);
    if (status) {
        const char *errors[] = {"", "", "ERR truncated", "ERR format", "ERR length", "ERR checksum"};
        puts(errors[status]);
        return status;
    }
    describe_record(&record);
    return 0;
}
