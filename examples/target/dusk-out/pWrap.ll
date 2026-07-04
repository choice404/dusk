; ModuleID = 'dusk'
target triple = "x86_64-pc-linux-gnu"

%Box = type { i64 }
%Wrap = type { { ptr, ptr } }

@vtable.Sized.Box = constant [1 x ptr] [ptr @thunk.Sized.Box.size]

declare void @cool_print_i64(i64)
declare void @cool_println_i64(i64)
declare void @cool_print_f64(double)
declare void @cool_println_f64(double)
declare void @cool_print_cstr(ptr)
declare void @cool_println_cstr(ptr)
declare void @cool_eprint_i64(i64)
declare void @cool_eprint_f64(double)
declare void @cool_eprint_cstr(ptr)
declare ptr @cool_alloc(i64)
declare void @cool_free(ptr)
declare ptr @cool_gen_alloc(i64)
declare void @cool_gen_free(ptr)
declare void @cool_gen_fault()
declare void @cool_null_fault()
declare void @cool_bounds_fault()
declare void @cool_shift_fault()
declare i64 @cool_pow_i64(i64, i64)
declare double @llvm.pow.f64(double, double)
declare float @llvm.pow.f32(float, float)
declare ptr @cool_debug_alloc(i64)
declare void @cool_debug_free(ptr)
declare i64 @cool_debug_leaks()
declare i64 @cool_debug_double_frees()
declare ptr @cool_thread_spawn(ptr, ptr)
declare i64 @cool_thread_join(ptr, i64)
declare i64 @cool_pool_submit(ptr, ptr)
declare ptr @cool_alloc_env(i64)
declare ptr @cool_task_new(ptr, i64, i64)
declare ptr @cool_task_frame(ptr)
declare void @cool_task_start(ptr)
declare void @cool_task_await(ptr, ptr, i64)
declare void @cool_task_return(ptr, ptr, i64)
declare ptr @cool_task_env_alloc(ptr, i64)
declare void @cool_future_take(ptr, i64, ptr, ptr)
declare void @cool_loop_run(ptr, i64, ptr, i64)
declare void @cool_task_state_fault()
declare ptr @cool_read_file(ptr)
declare i64 @cool_write_file(ptr, ptr)
declare ptr @cool_read_line()
declare ptr @cool_read_all()
declare double @cool_parse_float(ptr, ptr)

define i64 @Box.size(ptr %a0) {
entry:
  %t0 = getelementptr %Box, ptr %a0, i32 0, i32 0
  %t1 = load i64, ptr %t0
  ret i64 %t1
}

define i32 @main() {
entry:
  %t0 = insertvalue %Box undef, i64 9, 0
  %t1 = insertvalue %Wrap undef, { ptr, ptr } %t0, 0
  %t2 = alloca %Wrap
  store %Wrap %t1, ptr %t2
  %t3 = getelementptr %Wrap, ptr %t2, i32 0, i32 0
  %t4 = load { ptr, ptr }, ptr %t3
  %t5 = extractvalue { ptr, ptr } %t4, 0
  %t6 = extractvalue { ptr, ptr } %t4, 1
  %t7 = getelementptr [1 x ptr], ptr %t6, i64 0, i64 0
  %t8 = load ptr, ptr %t7
  %t9 = call i64 %t8(ptr %t5)
  call void @cool_println_i64(i64 %t9)
  %t10 = trunc i64 0 to i32
  ret i32 %t10
}

define i64 @thunk.Sized.Box.size(ptr %d) {
entry:
  %r = call i64 @Box.size(ptr %d)
  ret i64 %r
}

