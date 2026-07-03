; ModuleID = 'dusk'
target triple = "x86_64-pc-linux-gnu"

%Future$int64 = type { ptr, i64 }

@.str.0 = private unnamed_addr constant [1 x i8] c"\00"
@.str.1 = private unnamed_addr constant [31 x i8] c"the event loop could not start\00"
@.str.2 = private unnamed_addr constant [25 x i8] c"future already completed\00"
@.str.3 = private unnamed_addr constant [18 x i8] c"future is pending\00"

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

define i32 @main() {
entry:
  %t0 = call ptr @loop_init()
  %t1 = alloca ptr
  store ptr %t0, ptr %t1
  %t2 = load ptr, ptr %t1
  %t3 = call %Future$int64 @future_new$int64()
  %t4 = alloca %Future$int64
  store %Future$int64 %t3, ptr %t4
  %t5 = load %Future$int64, ptr %t4
  %t6 = call { i64, ptr } @try_poll$int64(%Future$int64 %t5)
  %t7 = extractvalue { i64, ptr } %t6, 0
  %t8 = alloca i64
  store i64 %t7, ptr %t8
  %t9 = extractvalue { i64, ptr } %t6, 1
  %t10 = alloca ptr
  store ptr %t9, ptr %t10
  %t11 = load ptr, ptr %t10
  %t12 = icmp ne ptr %t11, null
  br i1 %t12, label %L0, label %L1
L0:
  %t13 = load ptr, ptr %t10
  %t14 = icmp eq ptr %t13, null
  %t15 = select i1 %t14, ptr @.str.0, ptr %t13
  call void @cool_println_cstr(ptr %t15)
  br label %L1
L1:
  %t16 = load i64, ptr %t8
  call void @cool_println_i64(i64 %t16)
  %t17 = load %Future$int64, ptr %t4
  %t18 = call ptr @complete$int64(%Future$int64 %t17, i64 9, ptr null)
  %t19 = alloca ptr
  store ptr %t18, ptr %t19
  %t20 = load ptr, ptr %t19
  %t21 = load %Future$int64, ptr %t4
  %t22 = call { i64, ptr } @try_poll$int64(%Future$int64 %t21)
  %t23 = extractvalue { i64, ptr } %t22, 0
  %t24 = alloca i64
  store i64 %t23, ptr %t24
  %t25 = extractvalue { i64, ptr } %t22, 1
  %t26 = alloca ptr
  store ptr %t25, ptr %t26
  %t27 = load ptr, ptr %t26
  %t28 = load i64, ptr %t24
  call void @cool_println_i64(i64 %t28)
  call void @loop_free()
  %t29 = trunc i64 0 to i32
  ret i32 %t29
}

define ptr @loop_init() {
entry:
  %t0 = call i64 @cool_loop_init()
  %t1 = icmp eq i64 %t0, 1
  br i1 %t1, label %L0, label %L1
L0:
  ret ptr @.str.1
L1:
  ret ptr null
}

define void @loop_free() {
entry:
  call void @cool_loop_free()
  ret void
}

define ptr @complete$int64(%Future$int64 %a0, i64 %a1, ptr %a2) {
entry:
  %t0 = alloca %Future$int64
  store %Future$int64 %a0, ptr %t0
  %t1 = alloca i64
  store i64 %a1, ptr %t1
  %t2 = alloca ptr
  store ptr %a2, ptr %t2
  %t3 = getelementptr i64, ptr null, i64 1
  %t4 = ptrtoint ptr %t3 to i64
  %t5 = call ptr @cool_gen_alloc(i64 %t4)
  %t6 = alloca ptr
  store ptr %t5, ptr %t6
  %t7 = load ptr, ptr %t6
  %t8 = getelementptr i64, ptr %t7, i64 0
  %t9 = load i64, ptr %t1
  store i64 %t9, ptr %t8
  %t10 = getelementptr i64, ptr null, i64 1
  %t11 = ptrtoint ptr %t10 to i64
  %t12 = call ptr @cool_gen_alloc(i64 %t11)
  %t13 = alloca ptr
  store ptr %t12, ptr %t13
  %t14 = load ptr, ptr %t13
  %t15 = getelementptr ptr, ptr %t14, i64 0
  %t16 = load ptr, ptr %t2
  store ptr %t16, ptr %t15
  %t17 = getelementptr %Future$int64, ptr %t0, i32 0, i32 0
  %t18 = load ptr, ptr %t17
  %t19 = getelementptr %Future$int64, ptr %t0, i32 0, i32 1
  %t20 = load i64, ptr %t19
  %t21 = load ptr, ptr %t6
  %t22 = load ptr, ptr %t13
  %t23 = call i64 @cool_future_complete(ptr %t18, i64 %t20, ptr %t21, ptr %t22)
  %t24 = alloca i64
  store i64 %t23, ptr %t24
  %t25 = load ptr, ptr %t6
  call void @cool_gen_free(ptr %t25)
  %t26 = load ptr, ptr %t13
  call void @cool_gen_free(ptr %t26)
  %t27 = load i64, ptr %t24
  %t28 = icmp eq i64 %t27, 1
  br i1 %t28, label %L0, label %L1
L0:
  ret ptr @.str.2
L1:
  ret ptr null
}

define { i64, ptr } @try_poll$int64(%Future$int64 %a0) {
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
  %t15 = call i64 @cool_future_try(ptr %t10, i64 %t12, ptr %t13, ptr %t14)
  %t16 = alloca i64
  store i64 %t15, ptr %t16
  %t17 = load ptr, ptr %t4
  %t18 = getelementptr i64, ptr %t17, i64 0
  %t19 = load i64, ptr %t18
  %t20 = alloca i64
  store i64 %t19, ptr %t20
  %t21 = load ptr, ptr %t8
  %t22 = getelementptr ptr, ptr %t21, i64 0
  %t23 = load ptr, ptr %t22
  %t24 = alloca ptr
  store ptr %t23, ptr %t24
  %t25 = load ptr, ptr %t4
  call void @cool_gen_free(ptr %t25)
  %t26 = load ptr, ptr %t8
  call void @cool_gen_free(ptr %t26)
  %t27 = load i64, ptr %t16
  %t28 = icmp eq i64 %t27, 2
  br i1 %t28, label %L0, label %L1
L0:
  %t29 = load i64, ptr %t20
  %t30 = insertvalue { i64, ptr } undef, i64 %t29, 0
  %t31 = insertvalue { i64, ptr } %t30, ptr @.str.3, 1
  ret { i64, ptr } %t31
L1:
  %t32 = load i64, ptr %t20
  %t33 = load ptr, ptr %t24
  %t34 = insertvalue { i64, ptr } undef, i64 %t32, 0
  %t35 = insertvalue { i64, ptr } %t34, ptr %t33, 1
  ret { i64, ptr } %t35
}

define %Future$int64 @future_new$int64() {
entry:
  %t0 = getelementptr i64, ptr null, i64 1
  %t1 = ptrtoint ptr %t0 to i64
  %t2 = call ptr @cool_future_new(i64 %t1)
  %t3 = alloca ptr
  store ptr %t2, ptr %t3
  %t4 = load ptr, ptr %t3
  %t5 = insertvalue %Future$int64 undef, ptr %t4, 0
  %t6 = load ptr, ptr %t3
  %t7 = call i64 @cool_future_gen(ptr %t6)
  %t8 = insertvalue %Future$int64 %t5, i64 %t7, 1
  ret %Future$int64 %t8
}

