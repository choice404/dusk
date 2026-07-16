; ModuleID = 'dusk'
target triple = "x86_64-pc-linux-gnu"

declare void @cool_print_i64(i64)
declare void @cool_println_i64(i64)
declare void @cool_print_f64(double)
declare void @cool_println_f64(double)
declare void @cool_print_cstr(ptr)
declare void @cool_println_cstr(ptr)
declare void @cool_eprint_i64(i64)
declare void @cool_eprint_f64(double)
declare void @cool_eprint_cstr(ptr)
declare void @cool_print_bytes(ptr, i64)
declare void @cool_eprint_bytes(ptr, i64)
declare ptr @cool_alloc(i64)
declare void @cool_free(ptr)
declare ptr @cool_gen_alloc(i64)
declare void @cool_gen_free(ptr)
declare void @cool_gen_fault_at(ptr)
declare void @cool_null_fault_at(ptr)
declare void @cool_bounds_fault_at(ptr)
declare void @cool_shift_fault_at(ptr)
declare i64 @cool_pow_i64(i64, i64)
declare double @llvm.pow.f64(double, double)
declare float @llvm.pow.f32(float, float)
declare void @llvm.memcpy.p0.p0.i64(ptr, ptr, i64, i1)
declare i64 @strlen(ptr)
declare i64 @cool_str_eq(ptr, ptr)
declare ptr @cool_str_concat(ptr, ptr)
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
declare void @cool_gc_anchor(ptr)
declare ptr @cool_collect_alloc(i64)
declare void @cool_gc_collect()
declare i64 @cool_gc_live_blocks()
declare i64 @cool_gc_live_bytes()
declare i64 @cool_gc_collections()
declare void @cool_gc_add_region(ptr, i64)
declare void @cool_gc_del_region(ptr)
declare ptr @cool_read_file(ptr)
declare i64 @cool_write_file(ptr, ptr)
declare ptr @cool_read_line()
declare ptr @cool_read_all()
declare double @cool_parse_float(ptr, ptr)
declare i64 @call_n(i64, ptr, ptr)

define i64 @scale(i64 %a0, ptr %a1) {
entry:
  %t0 = alloca i64
  %t1 = alloca ptr
  store i64 %a0, ptr %t0
  store ptr %a1, ptr %t1
  %t2 = load i64, ptr %t0
  %t3 = load ptr, ptr %t1
  %t4 = getelementptr i64, ptr %t3, i64 0
  %t5 = load i64, ptr %t4
  %t6 = mul i64 %t2, %t5
  ret i64 %t6
}

define i32 @main() {
entry:
  %t0 = alloca i8
  %t2 = alloca ptr
  %t7 = alloca i64
  call void @cool_gc_anchor(ptr %t0)
  %t1 = call ptr @cool_gen_alloc(i64 8)
  store ptr %t1, ptr %t2
  %t3 = load ptr, ptr %t2
  %t4 = getelementptr i64, ptr %t3, i64 0
  store i64 10, ptr %t4
  %t5 = load ptr, ptr %t2
  %t6 = call i64 @call_n(i64 5, ptr %t5, ptr @scale)
  store i64 %t6, ptr %t7
  %t8 = load i64, ptr %t7
  call void @cool_println_i64(i64 %t8)
  %t9 = load ptr, ptr %t2
  call void @cool_gen_free(ptr %t9)
  %t10 = trunc i64 0 to i32
  ret i32 %t10
}

