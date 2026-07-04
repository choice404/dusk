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

define i32 @main() {
entry:
  %t0 = insertvalue { i64, i1 } undef, i64 1, 0
  %t1 = insertvalue { i64, i1 } %t0, i1 1, 1
  %t2 = insertvalue { i64, i1 } undef, i64 2, 0
  %t3 = insertvalue { i64, i1 } %t2, i1 0, 1
  %t4 = insertvalue { i64, i1 } undef, i64 3, 0
  %t5 = insertvalue { i64, i1 } %t4, i1 1, 1
  %t6 = insertvalue [3 x { i64, i1 }] undef, { i64, i1 } %t1, 0
  %t7 = insertvalue [3 x { i64, i1 }] %t6, { i64, i1 } %t3, 1
  %t8 = insertvalue [3 x { i64, i1 }] %t7, { i64, i1 } %t5, 2
  %t9 = alloca [3 x { i64, i1 }]
  store [3 x { i64, i1 }] %t8, ptr %t9
  %t10 = insertvalue { ptr, i64 } undef, ptr %t9, 0
  %t11 = insertvalue { ptr, i64 } %t10, i64 3, 1
  %t12 = alloca { ptr, i64 }
  store { ptr, i64 } %t11, ptr %t12
  %t13 = getelementptr { ptr, i64 }, ptr %t12, i32 0, i32 1
  %t14 = load i64, ptr %t13
  %t15 = icmp uge i64 0, %t14
  br i1 %t15, label %L0, label %L1
L0:
  call void @cool_bounds_fault()
  br label %L1
L1:
  %t16 = load ptr, ptr %t12
  %t17 = getelementptr { i64, i1 }, ptr %t16, i64 0
  %t18 = load { i64, i1 }, ptr %t17
  %t19 = extractvalue { i64, i1 } %t18, 0
  %t20 = alloca i64
  store i64 %t19, ptr %t20
  %t21 = extractvalue { i64, i1 } %t18, 1
  %t22 = alloca i1
  store i1 %t21, ptr %t22
  %t23 = getelementptr { ptr, i64 }, ptr %t12, i32 0, i32 1
  %t24 = load i64, ptr %t23
  %t25 = icmp uge i64 2, %t24
  br i1 %t25, label %L2, label %L3
L2:
  call void @cool_bounds_fault()
  br label %L3
L3:
  %t26 = load ptr, ptr %t12
  %t27 = getelementptr { i64, i1 }, ptr %t26, i64 2
  %t28 = load { i64, i1 }, ptr %t27
  %t29 = extractvalue { i64, i1 } %t28, 0
  %t30 = alloca i64
  store i64 %t29, ptr %t30
  %t31 = extractvalue { i64, i1 } %t28, 1
  %t32 = alloca i1
  store i1 %t31, ptr %t32
  %t33 = load i1, ptr %t22
  br i1 %t33, label %L4, label %L5
L4:
  %t34 = load i64, ptr %t20
  call void @cool_println_i64(i64 %t34)
  br label %L5
L5:
  %t35 = load i1, ptr %t32
  br i1 %t35, label %L6, label %L7
L6:
  %t36 = load i64, ptr %t30
  call void @cool_println_i64(i64 %t36)
  br label %L7
L7:
  %t37 = trunc i64 0 to i32
  ret i32 %t37
}

