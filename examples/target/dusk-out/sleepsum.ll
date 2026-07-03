; ModuleID = 'dusk'
target triple = "x86_64-pc-linux-gnu"

%Future$int64 = type { ptr, i64 }

@.str.0 = private unnamed_addr constant [31 x i8] c"the event loop could not start\00"

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
declare ptr @cool_debug_alloc(i64)
declare void @cool_debug_free(ptr)
declare i64 @cool_debug_leaks()
declare i64 @cool_debug_double_frees()
declare ptr @cool_thread_spawn(ptr, ptr)
declare i64 @cool_thread_join(ptr, i64)
declare i64 @cool_pool_submit(ptr, ptr)
declare ptr @cool_alloc_env(i64)
declare ptr @cool_read_file(ptr)
declare i64 @cool_write_file(ptr, ptr)
declare ptr @cool_read_line()
declare ptr @cool_read_all()
declare double @cool_parse_float(ptr, ptr)
declare ptr @cool_future_new(i64)
declare i64 @cool_future_gen(ptr)
declare i64 @cool_future_complete(ptr, i64, ptr, ptr)
declare i64 @cool_future_try(ptr, i64, ptr, ptr)
declare void @cool_future_wait(ptr, i64, ptr, ptr)
declare i64 @cool_future_await_ms(ptr, i64, i64, ptr, ptr)
declare void @cool_future_release(ptr, i64)
declare i64 @cool_loop_init()
declare void @cool_loop_free()
declare ptr @cool_timer_new(i64)

define i32 @main() {
entry:
  %t0 = call ptr @loop_init()
  %t1 = alloca ptr
  store ptr %t0, ptr %t1
  %t2 = load ptr, ptr %t1
  %t3 = call %Future$int64 @sleep_async(i64 2)
  %t4 = alloca %Future$int64
  store %Future$int64 %t3, ptr %t4
  %t5 = call %Future$int64 @sleep_async(i64 1)
  %t6 = alloca %Future$int64
  store %Future$int64 %t5, ptr %t6
  %t7 = call %Future$int64 @sleep_async(i64 3)
  %t8 = alloca %Future$int64
  store %Future$int64 %t7, ptr %t8
  %t9 = load %Future$int64, ptr %t4
  %t10 = call { i64, ptr } @await$int64(%Future$int64 %t9)
  %t11 = extractvalue { i64, ptr } %t10, 0
  %t12 = alloca i64
  store i64 %t11, ptr %t12
  %t13 = extractvalue { i64, ptr } %t10, 1
  %t14 = alloca ptr
  store ptr %t13, ptr %t14
  %t15 = load ptr, ptr %t14
  %t16 = load %Future$int64, ptr %t6
  %t17 = call { i64, ptr } @await$int64(%Future$int64 %t16)
  %t18 = extractvalue { i64, ptr } %t17, 0
  %t19 = alloca i64
  store i64 %t18, ptr %t19
  %t20 = extractvalue { i64, ptr } %t17, 1
  %t21 = alloca ptr
  store ptr %t20, ptr %t21
  %t22 = load ptr, ptr %t21
  %t23 = load %Future$int64, ptr %t8
  %t24 = call { i64, ptr } @await$int64(%Future$int64 %t23)
  %t25 = extractvalue { i64, ptr } %t24, 0
  %t26 = alloca i64
  store i64 %t25, ptr %t26
  %t27 = extractvalue { i64, ptr } %t24, 1
  %t28 = alloca ptr
  store ptr %t27, ptr %t28
  %t29 = load ptr, ptr %t28
  %t30 = load i64, ptr %t12
  %t31 = load i64, ptr %t19
  %t32 = add i64 %t30, %t31
  %t33 = load i64, ptr %t26
  %t34 = add i64 %t32, %t33
  %t35 = add i64 %t34, 42
  call void @cool_println_i64(i64 %t35)
  call void @loop_free()
  %t36 = trunc i64 0 to i32
  ret i32 %t36
}

define ptr @loop_init() {
entry:
  %t0 = call i64 @cool_loop_init()
  %t1 = icmp eq i64 %t0, 1
  br i1 %t1, label %L0, label %L1
L0:
  ret ptr @.str.0
L1:
  ret ptr null
}

define void @loop_free() {
entry:
  call void @cool_loop_free()
  ret void
}

define %Future$int64 @sleep_async(i64 %a0) {
entry:
  %t0 = alloca i64
  store i64 %a0, ptr %t0
  %t1 = load i64, ptr %t0
  %t2 = call ptr @cool_timer_new(i64 %t1)
  %t3 = call %Future$int64 @future_wrap$int64(ptr %t2)
  %t4 = alloca %Future$int64
  store %Future$int64 %t3, ptr %t4
  %t5 = load %Future$int64, ptr %t4
  ret %Future$int64 %t5
}

define %Future$int64 @future_wrap$int64(ptr %a0) {
entry:
  %t0 = alloca ptr
  store ptr %a0, ptr %t0
  %t1 = load ptr, ptr %t0
  %t2 = insertvalue %Future$int64 undef, ptr %t1, 0
  %t3 = load ptr, ptr %t0
  %t4 = call i64 @cool_future_gen(ptr %t3)
  %t5 = insertvalue %Future$int64 %t2, i64 %t4, 1
  ret %Future$int64 %t5
}

define { i64, ptr } @await$int64(%Future$int64 %a0) {
entry:
  %t0 = alloca %Future$int64
  store %Future$int64 %a0, ptr %t0
  %t1 = getelementptr i64, ptr null, i64 1
  %t2 = ptrtoint ptr %t1 to i64
  %t3 = call ptr @cool_gen_alloc(i64 %t2)
  %t4 = alloca ptr
  store ptr %t3, ptr %t4
  %t5 = getelementptr i64, ptr null, i64 1
  %t6 = ptrtoint ptr %t5 to i64
  %t7 = call ptr @cool_gen_alloc(i64 %t6)
  %t8 = alloca ptr
  store ptr %t7, ptr %t8
  %t9 = getelementptr %Future$int64, ptr %t0, i32 0, i32 0
  %t10 = load ptr, ptr %t9
  %t11 = getelementptr %Future$int64, ptr %t0, i32 0, i32 1
  %t12 = load i64, ptr %t11
  %t13 = load ptr, ptr %t4
  %t14 = load ptr, ptr %t8
  call void @cool_future_wait(ptr %t10, i64 %t12, ptr %t13, ptr %t14)
  %t15 = load ptr, ptr %t4
  %t16 = getelementptr i64, ptr %t15, i64 0
  %t17 = load i64, ptr %t16
  %t18 = alloca i64
  store i64 %t17, ptr %t18
  %t19 = load ptr, ptr %t8
  %t20 = getelementptr ptr, ptr %t19, i64 0
  %t21 = load ptr, ptr %t20
  %t22 = alloca ptr
  store ptr %t21, ptr %t22
  %t23 = load ptr, ptr %t4
  call void @cool_gen_free(ptr %t23)
  %t24 = load ptr, ptr %t8
  call void @cool_gen_free(ptr %t24)
  %t25 = load i64, ptr %t18
  %t26 = load ptr, ptr %t22
  %t27 = insertvalue { i64, ptr } undef, i64 %t25, 0
  %t28 = insertvalue { i64, ptr } %t27, ptr %t26, 1
  ret { i64, ptr } %t28
}

