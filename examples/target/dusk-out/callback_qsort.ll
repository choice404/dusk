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
declare void @qsort(ptr, i64, i64, ptr)

define i32 @lambda.0(ptr %a0, ptr %a1) {
entry:
  %t0 = alloca ptr
  %t1 = alloca ptr
  store ptr %a0, ptr %t0
  store ptr %a1, ptr %t1
  %t2 = load ptr, ptr %t0
  %t3 = getelementptr i64, ptr %t2, i64 0
  %t4 = load i64, ptr %t3
  %t5 = load ptr, ptr %t1
  %t6 = getelementptr i64, ptr %t5, i64 0
  %t7 = load i64, ptr %t6
  %t8 = icmp slt i64 %t4, %t7
  br i1 %t8, label %L0, label %L1
L0:
  %t9 = sub i64 0, 1
  %t10 = trunc i64 %t9 to i32
  ret i32 %t10
L1:
  %t11 = load ptr, ptr %t0
  %t12 = getelementptr i64, ptr %t11, i64 0
  %t13 = load i64, ptr %t12
  %t14 = load ptr, ptr %t1
  %t15 = getelementptr i64, ptr %t14, i64 0
  %t16 = load i64, ptr %t15
  %t17 = icmp sgt i64 %t13, %t16
  br i1 %t17, label %L2, label %L3
L2:
  %t18 = trunc i64 1 to i32
  ret i32 %t18
L3:
  %t19 = trunc i64 0 to i32
  ret i32 %t19
}

define i32 @main() {
entry:
  %t0 = alloca i8
  %t2 = alloca ptr
  %t14 = alloca i64
  call void @cool_gc_anchor(ptr %t0)
  %t1 = call ptr @cool_gen_alloc(i64 40)
  store ptr %t1, ptr %t2
  %t3 = load ptr, ptr %t2
  %t4 = getelementptr i64, ptr %t3, i64 0
  store i64 5, ptr %t4
  %t5 = load ptr, ptr %t2
  %t6 = getelementptr i64, ptr %t5, i64 1
  store i64 2, ptr %t6
  %t7 = load ptr, ptr %t2
  %t8 = getelementptr i64, ptr %t7, i64 2
  store i64 8, ptr %t8
  %t9 = load ptr, ptr %t2
  %t10 = getelementptr i64, ptr %t9, i64 3
  store i64 1, ptr %t10
  %t11 = load ptr, ptr %t2
  %t12 = getelementptr i64, ptr %t11, i64 4
  store i64 9, ptr %t12
  %t13 = load ptr, ptr %t2
  call void @qsort(ptr %t13, i64 5, i64 8, ptr @lambda.0)
  store i64 0, ptr %t14
  br label %L0
L0:
  %t15 = load i64, ptr %t14
  %t16 = icmp slt i64 %t15, 5
  br i1 %t16, label %L1, label %L2
L1:
  %t17 = load i64, ptr %t14
  %t18 = load ptr, ptr %t2
  %t19 = getelementptr i64, ptr %t18, i64 %t17
  %t20 = load i64, ptr %t19
  call void @cool_println_i64(i64 %t20)
  %t21 = load i64, ptr %t14
  %t22 = add i64 %t21, 1
  store i64 %t22, ptr %t14
  br label %L0
L2:
  %t23 = load ptr, ptr %t2
  call void @cool_gen_free(ptr %t23)
  %t24 = trunc i64 0 to i32
  ret i32 %t24
}

