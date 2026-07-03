; ModuleID = 'dusk'
target triple = "x86_64-pc-linux-gnu"

%Future$int64 = type { ptr, i64 }

@.str.0 = private unnamed_addr constant [1 x i8] c"\00"
@.str.1 = private unnamed_addr constant [31 x i8] c"the event loop could not start\00"
@.str.2 = private unnamed_addr constant [25 x i8] c"future already completed\00"

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
  %t6 = call ptr @complete$int64(%Future$int64 %t5, i64 4, ptr null)
  %t7 = alloca ptr
  store ptr %t6, ptr %t7
  %t8 = load ptr, ptr %t7
  %t9 = load %Future$int64, ptr %t4
  %t10 = call ptr @complete$int64(%Future$int64 %t9, i64 5, ptr null)
  %t11 = alloca ptr
  store ptr %t10, ptr %t11
  %t12 = load ptr, ptr %t11
  %t13 = icmp ne ptr %t12, null
  br i1 %t13, label %L0, label %L1
L0:
  %t14 = load ptr, ptr %t11
  %t15 = icmp eq ptr %t14, null
  %t16 = select i1 %t15, ptr @.str.0, ptr %t14
  call void @cool_println_cstr(ptr %t16)
  br label %L1
L1:
  %t17 = load %Future$int64, ptr %t4
  %t18 = call { i64, ptr } @await$int64(%Future$int64 %t17)
  %t19 = extractvalue { i64, ptr } %t18, 0
  %t20 = alloca i64
  store i64 %t19, ptr %t20
  %t21 = extractvalue { i64, ptr } %t18, 1
  %t22 = alloca ptr
  store ptr %t21, ptr %t22
  %t23 = load ptr, ptr %t22
  %t24 = load i64, ptr %t20
  call void @cool_println_i64(i64 %t24)
  call void @loop_free()
  %t25 = trunc i64 0 to i32
  ret i32 %t25
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

