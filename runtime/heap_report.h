/* The heap report of a --heap-sites build, included by heap_gauge.h in place of the
   plain one. Not a standalone header: it reads the heap lock, the registry, the
   counters, and the chain table from the files that include it.

   The report walks the registry under the heap lock, keeps every block whose size
   word has no parked bit, groups the blocks by the chain of call sites recorded
   when each was allocated, and prints the groups by bytes, largest first. It prints
   at exit when anything is still live, and whenever the program calls heap_report.
   Three environment variables, read once at startup, adjust it:

     DUSK_HEAP_EXIT=0      no report at exit
     DUSK_HEAP_DEPTH=1..8  how many call sites of a chain the report groups by,
                           eight by default; 1 groups by the line alone
     DUSK_HEAP_SIGNAL=USR1 a signal, by name or number, that prints a report; off
                           unless set

   The report holds the heap lock while it prints, so a report written into a pipe
   nobody reads stalls every allocation. A fault aborts and prints no report. */

#include <signal.h>

static int64_t cool_report_depth = COOL_SITE_CHAIN;

/* One live block as the report reads it, and one group of them. */
typedef struct {
    int64_t chain;
    int64_t size;
} cool_live_rec;

typedef struct {
    int64_t chain;
    int64_t count;
    int64_t bytes;
} cool_site_group;

/* How many locations of a chain the report reads, the chain cut to the depth. */
static int64_t cool_chain_cut(int64_t id) {
    if (id <= 0 || id >= cool_chain_n) {
        return 0;
    }
    int64_t n = cool_chains[id].n;
    return n < cool_report_depth ? n : cool_report_depth;
}

/* Orders two chains by their cut locations, pointer by pointer, so equal cut
   chains sit together for the fold. */
static int cool_chain_key_cmp(int64_t a, int64_t b) {
    int64_t na = cool_chain_cut(a);
    int64_t nb = cool_chain_cut(b);
    int64_t n = na < nb ? na : nb;
    for (int64_t i = 0; i < n; i++) {
        uintptr_t pa = (uintptr_t)cool_chains[a].loc[i];
        uintptr_t pb = (uintptr_t)cool_chains[b].loc[i];
        if (pa != pb) {
            return pa < pb ? -1 : 1;
        }
    }
    return (na > nb) - (na < nb);
}

static int cool_live_cmp(const void *x, const void *y) {
    return cool_chain_key_cmp(((const cool_live_rec *)x)->chain, ((const cool_live_rec *)y)->chain);
}

/* Orders two location texts, a missing one last. */
static int cool_loc_text_cmp(const char *a, const char *b) {
    if (a == b) {
        return 0;
    }
    if (!a) {
        return 1;
    }
    if (!b) {
        return -1;
    }
    return strcmp(a, b);
}

/* Orders the chains of two groups by their texts, so the order of the report never
   depends on where the constants landed in memory. */
static int cool_chain_text_cmp(int64_t a, int64_t b) {
    int64_t na = cool_chain_cut(a);
    int64_t nb = cool_chain_cut(b);
    int64_t n = na > nb ? na : nb;
    for (int64_t i = 0; i < n; i++) {
        const char *ta = i < na ? cool_chains[a].loc[i] : NULL;
        const char *tb = i < nb ? cool_chains[b].loc[i] : NULL;
        int c = cool_loc_text_cmp(ta, tb);
        if (c != 0) {
            return c;
        }
    }
    return 0;
}

/* The report's order: bytes, then blocks, largest first, then the chain's text. */
static int cool_group_cmp(const void *x, const void *y) {
    const cool_site_group *a = x;
    const cool_site_group *b = y;
    if (a->bytes != b->bytes) {
        return a->bytes > b->bytes ? -1 : 1;
    }
    if (a->count != b->count) {
        return a->count > b->count ? -1 : 1;
    }
    return cool_chain_text_cmp(a->chain, b->chain);
}

/* Folds sorted records into groups of one cut chain, in place over a groups array
   as long as the records, and returns the group count. */
static int64_t cool_fold_groups(cool_live_rec *recs, int64_t m, cool_site_group *groups) {
    int64_t g = 0;
    for (int64_t i = 0; i < m; i++) {
        if (g > 0 && cool_chain_key_cmp(groups[g - 1].chain, recs[i].chain) == 0) {
            groups[g - 1].count++;
            groups[g - 1].bytes += recs[i].size;
            continue;
        }
        groups[g].chain = recs[i].chain;
        groups[g].count = 1;
        groups[g].bytes = recs[i].size;
        g++;
    }
    return g;
}

/* Prints one group: its counts and innermost site, then each outer site of its cut
   chain on a line of its own. */
static void cool_print_group(const cool_site_group *grp) {
    int64_t n = cool_chain_cut(grp->chain);
    const char *site = n > 0 && cool_chains[grp->chain].loc[0] ? cool_chains[grp->chain].loc[0] : "(no site)";
    fprintf(stderr, "%7" PRId64 " %-6s %8" PRId64 " %-5s  %s\n", grp->count, cool_heap_plural(grp->count, "block", "blocks"),
            grp->bytes, cool_heap_plural(grp->bytes, "byte", "bytes"), site);
    for (int64_t i = 1; i < n; i++) {
        const char *outer = cool_chains[grp->chain].loc[i];
        fprintf(stderr, "        from %s\n", outer ? outer : "(no site)");
    }
}

/* Prints the totals line, and the drift line when a walk disagrees with the
   counters, which every report checks. */
static void cool_print_totals(int64_t blocks, int64_t bytes, int64_t groups) {
    fprintf(stderr, "dusk heap: %" PRId64 " %s, %" PRId64 " %s live", blocks, cool_heap_plural(blocks, "block", "blocks"),
            bytes, cool_heap_plural(bytes, "byte", "bytes"));
    if (groups >= 0) {
        fprintf(stderr, ", %" PRId64 " %s", groups, cool_heap_plural(groups, "site", "sites"));
    }
    fputc('\n', stderr);
}

static void cool_print_drift(int64_t blocks, int64_t bytes) {
    if (blocks != cool_heap_blocks || bytes != cool_heap_bytes) {
        fprintf(stderr, "dusk heap: counter drift, blocks %" PRId64 " against %" PRId64 ", bytes %" PRId64 " against %" PRId64 "\n",
                blocks, cool_heap_blocks, bytes, cool_heap_bytes);
    }
}

/* The report itself, with the heap lock held. A record array that cannot be
   allocated degrades the report to its totals line. */
static void cool_heap_report_locked(void) {
    int64_t n = cool_reg_n;
    cool_live_rec *recs = malloc((size_t)(n > 0 ? n : 1) * sizeof(cool_live_rec));
    cool_site_group *groups = malloc((size_t)(n > 0 ? n : 1) * sizeof(cool_site_group));
    int64_t blocks = 0;
    int64_t bytes = 0;
    for (int64_t i = 0; i < n; i++) {
        void *p = cool_reg_ptr[i];
        int64_t size = *COOL_GEN_SIZE_WORD(p);
        if (size & COOL_GEN_FREED_TAG) {
            continue;
        }
        int64_t counted = size > 0 ? size : 0;
        if (recs) {
            recs[blocks].chain = *COOL_GEN_CHAIN_WORD(p);
            recs[blocks].size = counted;
        }
        blocks++;
        bytes += counted;
    }
    if (!recs || !groups) {
        cool_print_totals(blocks, bytes, -1);
        cool_print_drift(blocks, bytes);
        free(recs);
        free(groups);
        return;
    }
    qsort(recs, (size_t)blocks, sizeof(cool_live_rec), cool_live_cmp);
    int64_t g = cool_fold_groups(recs, blocks, groups);
    qsort(groups, (size_t)g, sizeof(cool_site_group), cool_group_cmp);
    cool_print_totals(blocks, bytes, g);
    for (int64_t i = 0; i < g; i++) {
        cool_print_group(&groups[i]);
    }
    cool_print_drift(blocks, bytes);
    free(recs);
    free(groups);
}

/* Prints the live blocks grouped by allocation site to stderr, after flushing
   stdout so the report lands after everything the program printed before it. */
void cool_heap_report(void) {
    fflush(stdout);
    pthread_mutex_lock(&cool_heap_lock);
    cool_heap_report_locked();
    pthread_mutex_unlock(&cool_heap_lock);
}

/* The report at exit, printed only when a block is still live, so a program that
   frees everything prints exactly what it prints without the flag. */
static void cool_heap_exit_report(void) {
    pthread_mutex_lock(&cool_heap_lock);
    int64_t live = cool_heap_blocks;
    pthread_mutex_unlock(&cool_heap_lock);
    if (live > 0) {
        cool_heap_report();
    }
}

static sigset_t cool_heap_sigset;

/* The report thread's loop. sigwait takes the signal off the blocked set, so no
   handler ever runs and nothing here is in signal context. */
static void *cool_heap_signal_loop(void *arg) {
    (void)arg;
    for (;;) {
        int sig = 0;
        if (sigwait(&cool_heap_sigset, &sig) == 0) {
            cool_heap_report();
        }
    }
    return NULL;
}

/* The signal DUSK_HEAP_SIGNAL names, by number or by name with or without SIG,
   or 0 for a name it does not know. */
static int cool_heap_signal_number(const char *spec) {
    if (spec[0] >= '0' && spec[0] <= '9') {
        return atoi(spec);
    }
    const char *name = strncmp(spec, "SIG", 3) == 0 ? spec + 3 : spec;
    if (strcmp(name, "USR1") == 0) return SIGUSR1;
    if (strcmp(name, "USR2") == 0) return SIGUSR2;
    if (strcmp(name, "HUP") == 0) return SIGHUP;
    if (strcmp(name, "INT") == 0) return SIGINT;
    if (strcmp(name, "QUIT") == 0) return SIGQUIT;
    if (strcmp(name, "TERM") == 0) return SIGTERM;
    return 0;
}

/* Blocks the report signal in the main thread before any other thread exists, so
   every thread inherits the mask, and starts the one detached thread that waits
   for it. A name it does not know is said once and leaves the signal alone. */
static void cool_heap_signal_start(const char *spec) {
    int sig = cool_heap_signal_number(spec);
    if (sig <= 0 || sig >= 65) {
        fprintf(stderr, "dusk heap: DUSK_HEAP_SIGNAL '%s' names no signal; no report signal\n", spec);
        return;
    }
    sigemptyset(&cool_heap_sigset);
    sigaddset(&cool_heap_sigset, sig);
    pthread_sigmask(SIG_BLOCK, &cool_heap_sigset, NULL);
    pthread_attr_t attr;
    pthread_attr_init(&attr);
    pthread_attr_setdetachstate(&attr, PTHREAD_CREATE_DETACHED);
    pthread_t t;
    if (pthread_create(&t, &attr, cool_heap_signal_loop, NULL) != 0) {
        fputs("dusk heap: could not start the report signal thread\n", stderr);
    }
    pthread_attr_destroy(&attr);
}

/* Reads the three variables once, before main, and registers the exit report. */
__attribute__((constructor)) static void cool_heap_sites_init(void) {
    const char *depth = getenv("DUSK_HEAP_DEPTH");
    if (depth && depth[0] >= '1' && depth[0] <= '8' && depth[1] == 0) {
        cool_report_depth = depth[0] - '0';
    }
    const char *at_exit = getenv("DUSK_HEAP_EXIT");
    if (!at_exit || strcmp(at_exit, "0") != 0) {
        atexit(cool_heap_exit_report);
    }
    const char *sig = getenv("DUSK_HEAP_SIGNAL");
    if (sig && sig[0]) {
        cool_heap_signal_start(sig);
    }
}
