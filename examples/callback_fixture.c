/* A C source compiled into the program through the dusk @csource directive.
   call_n invokes the callback n times, threading a user data pointer through
   each call, and folds the results. The dusk side in callback_fixture_repeat.dusk
   binds call_n and passes a dusk function as the callback. */
long call_n(long n, void *user, long (*fn)(long, void *)) {
    long acc = 0;
    for (long i = 0; i < n; i++) {
        acc += fn(i, user);
    }
    return acc;
}
