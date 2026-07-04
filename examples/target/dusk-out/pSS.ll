; ModuleID = 'dusk'
target triple = "x86_64-pc-linux-gnu"

%Box = type { i64 }

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

define i64 @take({ ptr, i64 } %a0) {
entry:
  %t0 = alloca { ptr, i64 }
  store { ptr, i64 } %a0, ptr %t0
  %t1 = getelementptr { ptr, i64 }, ptr %t0, i32 0, i32 1
  %t2 = load i64, ptr %t1
  %t3 = icmp uge i64 0, %t2
  br i1 %t3, label %L0, label %L1
L0:
  call void @cool_bounds_fault()
  br label %L1
L1:
  %t4 = load ptr, ptr %t0
  %t5 = getelementptr { ptr, ptr }, ptr %t4, i64 0
  %t6 = load { ptr, ptr }, ptr %t5
  %t7 = extractvalue { ptr, ptr } %t6, 0
  %t8 = extractvalue { ptr, ptr } %t6, 1
  %t9 = getelementptr [1 x ptr], ptr %t8, i64 0, i64 0
  %t10 = load ptr, ptr %t9
  %t11 = call i64 %t10(ptr %t7)
  %t12 = getelementptr { ptr, i64 }, ptr %t0, i32 0, i32 1
  %t13 = load i64, ptr %t12
  %t14 = icmp uge i64 1, %t13
  br i1 %t14, label %L2, label %L3
L2:
  call void @cool_bounds_fault()
  br label %L3
L3:
  %t15 = load ptr, ptr %t0
  %t16 = getelementptr { ptr, ptr }, ptr %t15, i64 1
  %t17 = load { ptr, ptr }, ptr %t16
  %t18 = extractvalue { ptr, ptr } %t17, 0
  %t19 = extractvalue { ptr, ptr } %t17, 1
  %t20 = getelementptr [1 x ptr], ptr %t19, i64 0, i64 0
  %t21 = load ptr, ptr %t20
  %t22 = call i64 %t21(ptr %t18)
  %t23 = add i64 %t11, %t22
  ret i64 %t23
}

define i32 @main() {
entry:
  %t0 = insertvalue %Box undef, i64 10, 0
  %t1 = insertvalue %Box undef, i64 20, 0
  %t2 = insertvalue [2 x %Box] undef, %Box %t0, 0
  %t3 = insertvalue [2 x %Box] %t2, %Box %t1, 1
  %t4 = alloca [2 x %Box]
  store [2 x %Box] %t3, ptr %t4
  %t5 = getelementptr [2 x %Box], ptr %t4, i64 0, i64 0
  %t6 = icmp ugt i64 0, 2
  br i1 %t6, label %L0, label %L1
L0:
  call void @cool_bounds_fault()
  br label %L1
L1:
  %t7 = icmp ugt i64 2, 2
  br i1 %t7, label %L2, label %L3
L2:
  call void @cool_bounds_fault()
  br label %L3
L3:
  %t8 = sub i64 2, 0
  %t9 = getelementptr %Box, ptr %t5, i64 0
  %t10 = insertvalue { ptr, i64 } undef, ptr %t9, 0
  %t11 = insertvalue { ptr, i64 } %t10, i64 %t8, 1
  %t12 = call i64 @take({ ptr, i64 } %t11)
  call void @cool_println_i64(i64 %t12)
  %t13 = trunc i64 0 to i32
  ret i32 %t13
}

define i64 @thunk.Sized.Box.size(ptr %d) {
entry:
  %r = call i64 @Box.size(ptr %d)
  ret i64 %r
}

