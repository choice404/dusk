/* The generational heap's gauge, included by runtime.c after the registry. Not a
   standalone header: it reads the heap lock, the registry, and the three counters
   from the file that includes it.

   The counters are written only on paths that already hold the heap lock, so
   keeping them costs two adds per allocation and two per free. blocks and bytes
   are the live blocks and the bytes their tenants asked for; held is the
   capacity of every block the heap has minted, live or parked, which is the
   memory the heap keeps from libc and what resident size follows. Each read takes
   the lock, so a value is one consistent moment of the heap. */

int64_t cool_heap_live_blocks(void) {
    pthread_mutex_lock(&cool_heap_lock);
    int64_t n = cool_heap_blocks;
    pthread_mutex_unlock(&cool_heap_lock);
    return n;
}

int64_t cool_heap_live_bytes(void) {
    pthread_mutex_lock(&cool_heap_lock);
    int64_t n = cool_heap_bytes;
    pthread_mutex_unlock(&cool_heap_lock);
    return n;
}

int64_t cool_heap_held_bytes(void) {
    pthread_mutex_lock(&cool_heap_lock);
    int64_t n = cool_heap_held;
    pthread_mutex_unlock(&cool_heap_lock);
    return n;
}

/* "block" or "blocks", "byte" or "bytes", so a count of one reads as English. */
static const char *cool_heap_plural(int64_t n, const char *one, const char *many) {
    return n == 1 ? one : many;
}

/* Prints the heap's live totals to stderr. The totals are recomputed from a walk of
   the registry, every entry whose size word has no parked bit, and compared with
   the counters, so every report is also the standing check that the counters
   agree with the heap: a disagreement prints one drift line. A build without
   allocation sites has no per site table, so the report says how to get one.
   stdout is flushed first so the report lands after everything the program
   printed before it. The heap lock is held across the walk and the print. */
void cool_heap_report(void) {
    fflush(stdout);
    pthread_mutex_lock(&cool_heap_lock);
    int64_t blocks = 0;
    int64_t bytes = 0;
    for (int64_t i = 0; i < cool_reg_n; i++) {
        int64_t size = *COOL_GEN_SIZE_WORD(cool_reg_ptr[i]);
        if (size & COOL_GEN_FREED_TAG) {
            continue;
        }
        blocks++;
        if (size > 0) {
            bytes += size;
        }
    }
    fprintf(stderr, "dusk heap: %" PRId64 " %s, %" PRId64 " %s live\n", blocks,
            cool_heap_plural(blocks, "block", "blocks"), bytes, cool_heap_plural(bytes, "byte", "bytes"));
    if (blocks != cool_heap_blocks || bytes != cool_heap_bytes) {
        fprintf(stderr, "dusk heap: counter drift, blocks %" PRId64 " against %" PRId64 ", bytes %" PRId64 " against %" PRId64 "\n",
                blocks, cool_heap_blocks, bytes, cool_heap_bytes);
    }
    fputs("dusk heap: build with --heap-sites to see allocation sites\n", stderr);
    pthread_mutex_unlock(&cool_heap_lock);
}
