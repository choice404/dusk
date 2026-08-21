/* Outside the checkout: a dependency may not reach a C source the lock does not
   stand behind, however real the file is. */
int evil_ctor(void) { return 1; }
