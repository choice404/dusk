/* A signed overflow, undefined in C, which UndefinedBehaviorSanitizer reports. */
int c_add_one(int x) {
    return x + 1;
}
