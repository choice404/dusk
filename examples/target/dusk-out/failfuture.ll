; ModuleID = 'dusk'
target triple = "x86_64-pc-linux-gnu"

%StringBuilder = type { ptr, i64, i64 }
%Future$int64 = type { ptr, i64 }

@.str.0 = private unnamed_addr constant [3 x i8] c"xy\00"
@.str.1 = private unnamed_addr constant [31 x i8] c"the thread pool is not running\00"
@.str.2 = private unnamed_addr constant [1 x i8] c"\00"
@.str.3 = private unnamed_addr constant [32 x i8] c"the thread pool could not start\00"
@.str.4 = private unnamed_addr constant [30 x i8] c"base must be between 2 and 36\00"
@.str.5 = private unnamed_addr constant [23 x i8] c"invalid digit for base\00"
@.str.6 = private unnamed_addr constant [17 x i8] c"integer overflow\00"
@.str.7 = private unnamed_addr constant [19 x i8] c"no digits to parse\00"
@.str.8 = private unnamed_addr constant [31 x i8] c"the event loop could not start\00"
@.str.9 = private unnamed_addr constant [25 x i8] c"future already completed\00"

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
declare i64 @cool_pool_start(i64)
declare void @cool_pool_shutdown()
declare i64 @cool_ncpu()
declare ptr @cool_future_new(i64)
declare i64 @cool_future_gen(ptr)
declare i64 @cool_future_complete(ptr, i64, ptr, ptr)
declare i64 @cool_future_try(ptr, i64, ptr, ptr)
declare void @cool_future_wait(ptr, i64, ptr, ptr)
declare i64 @cool_future_await_ms(ptr, i64, i64, ptr, ptr)
declare void @cool_future_release(ptr, i64)
declare i64 @cool_loop_init()
declare void @cool_loop_free()

define void @lambda.0(ptr %env) {
entry:
  %t0 = getelementptr { %Future$int64 }, ptr %env, i32 0, i32 0
  %t1 = load %Future$int64, ptr %t0
  %t2 = alloca %Future$int64
  store %Future$int64 %t1, ptr %t2
  %t3 = call { i64, ptr } @parse_int(ptr @.str.0)
  %t4 = extractvalue { i64, ptr } %t3, 0
  %t5 = alloca i64
  store i64 %t4, ptr %t5
  %t6 = extractvalue { i64, ptr } %t3, 1
  %t7 = alloca ptr
  store ptr %t6, ptr %t7
  %t8 = load ptr, ptr %t7
  %t9 = load %Future$int64, ptr %t2
  %t10 = load i64, ptr %t5
  %t11 = load ptr, ptr %t7
  %t12 = call ptr @complete$int64(%Future$int64 %t9, i64 %t10, ptr %t11)
  %t13 = alloca ptr
  store ptr %t12, ptr %t13
  %t14 = load ptr, ptr %t13
  ret void
}

define i32 @main() {
entry:
  %t0 = call ptr @loop_init()
  %t1 = alloca ptr
  store ptr %t0, ptr %t1
  %t2 = load ptr, ptr %t1
  %t3 = call ptr @pool_start(i64 1)
  %t4 = alloca ptr
  store ptr %t3, ptr %t4
  %t5 = load ptr, ptr %t4
  %t6 = call %Future$int64 @future_new$int64()
  %t7 = alloca %Future$int64
  store %Future$int64 %t6, ptr %t7
  %t8 = getelementptr { %Future$int64 }, ptr null, i64 1
  %t9 = ptrtoint ptr %t8 to i64
  %t10 = call ptr @cool_alloc_env(i64 %t9)
  %t11 = load %Future$int64, ptr %t7
  %t12 = getelementptr { %Future$int64 }, ptr %t10, i32 0, i32 0
  store %Future$int64 %t11, ptr %t12
  %t13 = call i64 @cool_pool_submit(ptr @lambda.0, ptr %t10)
  %t14 = icmp ne i64 %t13, 0
  %t15 = select i1 %t14, ptr @.str.1, ptr null
  %t16 = alloca ptr
  store ptr %t15, ptr %t16
  %t17 = load ptr, ptr %t16
  %t18 = load %Future$int64, ptr %t7
  %t19 = call { i64, ptr } @await$int64(%Future$int64 %t18)
  %t20 = extractvalue { i64, ptr } %t19, 0
  %t21 = alloca i64
  store i64 %t20, ptr %t21
  %t22 = extractvalue { i64, ptr } %t19, 1
  %t23 = alloca ptr
  store ptr %t22, ptr %t23
  %t24 = load ptr, ptr %t23
  %t25 = icmp ne ptr %t24, null
  br i1 %t25, label %L0, label %L1
L0:
  %t26 = load ptr, ptr %t23
  %t27 = icmp eq ptr %t26, null
  %t28 = select i1 %t27, ptr @.str.2, ptr %t26
  call void @cool_println_cstr(ptr %t28)
  br label %L1
L1:
  %t29 = load i64, ptr %t21
  call void @cool_println_i64(i64 %t29)
  call void @pool_shutdown()
  call void @loop_free()
  %t30 = trunc i64 0 to i32
  ret i32 %t30
}

define ptr @pool_start(i64 %a0) {
entry:
  %t0 = alloca i64
  store i64 %a0, ptr %t0
  %t1 = load i64, ptr %t0
  %t2 = call i64 @cool_pool_start(i64 %t1)
  %t3 = icmp eq i64 %t2, 1
  br i1 %t3, label %L0, label %L1
L0:
  ret ptr @.str.3
L1:
  ret ptr null
}

define void @pool_shutdown() {
entry:
  call void @cool_pool_shutdown()
  ret void
}

define i64 @ncpu() {
entry:
  %t0 = call i64 @cool_ncpu()
  ret i64 %t0
}

define i64 @str_len(ptr %a0) {
entry:
  %t0 = alloca ptr
  store ptr %a0, ptr %t0
  %t1 = alloca i64
  store i64 0, ptr %t1
  br label %L0
L0:
  %t2 = load i64, ptr %t1
  %t3 = load ptr, ptr %t0
  %t4 = getelementptr i8, ptr %t3, i64 %t2
  %t5 = load i8, ptr %t4
  %t6 = trunc i64 0 to i8
  %t7 = icmp ne i8 %t5, %t6
  br i1 %t7, label %L1, label %L2
L1:
  %t8 = load i64, ptr %t1
  %t9 = add i64 %t8, 1
  store i64 %t9, ptr %t1
  br label %L0
L2:
  %t10 = load i64, ptr %t1
  ret i64 %t10
}

define i1 @str_eq(ptr %a0, ptr %a1) {
entry:
  %t0 = alloca ptr
  store ptr %a0, ptr %t0
  %t1 = alloca ptr
  store ptr %a1, ptr %t1
  %t2 = alloca i64
  store i64 0, ptr %t2
  br label %L0
L0:
  %t3 = load i64, ptr %t2
  %t4 = load ptr, ptr %t0
  %t5 = getelementptr i8, ptr %t4, i64 %t3
  %t6 = load i8, ptr %t5
  %t7 = trunc i64 0 to i8
  %t8 = icmp ne i8 %t6, %t7
  br i1 %t8, label %L1, label %L2
L1:
  %t9 = load i64, ptr %t2
  %t10 = load ptr, ptr %t0
  %t11 = getelementptr i8, ptr %t10, i64 %t9
  %t12 = load i8, ptr %t11
  %t13 = load i64, ptr %t2
  %t14 = load ptr, ptr %t1
  %t15 = getelementptr i8, ptr %t14, i64 %t13
  %t16 = load i8, ptr %t15
  %t17 = icmp ne i8 %t12, %t16
  br i1 %t17, label %L3, label %L4
L3:
  ret i1 0
L4:
  %t18 = load i64, ptr %t2
  %t19 = add i64 %t18, 1
  store i64 %t19, ptr %t2
  br label %L0
L2:
  %t20 = load i64, ptr %t2
  %t21 = load ptr, ptr %t1
  %t22 = getelementptr i8, ptr %t21, i64 %t20
  %t23 = load i8, ptr %t22
  %t24 = trunc i64 0 to i8
  %t25 = icmp eq i8 %t23, %t24
  ret i1 %t25
}

define i64 @digit_val__string_2(i64 %a0, i64 %a1) {
entry:
  %t0 = alloca i64
  store i64 %a0, ptr %t0
  %t1 = alloca i64
  store i64 %a1, ptr %t1
  %t2 = sub i64 0, 1
  %t3 = alloca i64
  store i64 %t2, ptr %t3
  %t4 = load i64, ptr %t0
  %t5 = icmp sge i64 %t4, 48
  br i1 %t5, label %L0, label %L1
L0:
  %t6 = load i64, ptr %t0
  %t7 = icmp sle i64 %t6, 57
  br i1 %t7, label %L2, label %L3
L2:
  %t8 = load i64, ptr %t0
  %t9 = sub i64 %t8, 48
  store i64 %t9, ptr %t3
  br label %L3
L3:
  br label %L1
L1:
  %t10 = load i64, ptr %t0
  %t11 = icmp sge i64 %t10, 65
  br i1 %t11, label %L4, label %L5
L4:
  %t12 = load i64, ptr %t0
  %t13 = icmp sle i64 %t12, 90
  br i1 %t13, label %L6, label %L7
L6:
  %t14 = load i64, ptr %t0
  %t15 = sub i64 %t14, 55
  store i64 %t15, ptr %t3
  br label %L7
L7:
  br label %L5
L5:
  %t16 = load i64, ptr %t0
  %t17 = icmp sge i64 %t16, 97
  br i1 %t17, label %L8, label %L9
L8:
  %t18 = load i64, ptr %t0
  %t19 = icmp sle i64 %t18, 122
  br i1 %t19, label %L10, label %L11
L10:
  %t20 = load i64, ptr %t0
  %t21 = sub i64 %t20, 87
  store i64 %t21, ptr %t3
  br label %L11
L11:
  br label %L9
L9:
  %t22 = load i64, ptr %t3
  %t23 = load i64, ptr %t1
  %t24 = icmp sge i64 %t22, %t23
  br i1 %t24, label %L12, label %L13
L12:
  %t25 = sub i64 0, 1
  ret i64 %t25
L13:
  %t26 = load i64, ptr %t3
  ret i64 %t26
}

define { i64, ptr } @parse_int_radix(ptr %a0, i64 %a1) {
entry:
  %t0 = alloca ptr
  store ptr %a0, ptr %t0
  %t1 = alloca i64
  store i64 %a1, ptr %t1
  %t2 = load i64, ptr %t1
  %t3 = icmp slt i64 %t2, 2
  br i1 %t3, label %L0, label %L1
L0:
  %t4 = insertvalue { i64, ptr } undef, i64 0, 0
  %t5 = insertvalue { i64, ptr } %t4, ptr @.str.4, 1
  ret { i64, ptr } %t5
L1:
  %t6 = load i64, ptr %t1
  %t7 = icmp sgt i64 %t6, 36
  br i1 %t7, label %L2, label %L3
L2:
  %t8 = insertvalue { i64, ptr } undef, i64 0, 0
  %t9 = insertvalue { i64, ptr } %t8, ptr @.str.4, 1
  ret { i64, ptr } %t9
L3:
  %t10 = alloca i64
  store i64 0, ptr %t10
  %t11 = alloca i1
  store i1 0, ptr %t11
  %t12 = load ptr, ptr %t0
  %t13 = getelementptr i8, ptr %t12, i64 0
  %t14 = load i8, ptr %t13
  %t15 = zext i8 %t14 to i64
  %t16 = alloca i64
  store i64 %t15, ptr %t16
  %t17 = load i64, ptr %t16
  %t18 = icmp eq i64 %t17, 45
  br i1 %t18, label %L4, label %L5
L4:
  store i1 1, ptr %t11
  store i64 1, ptr %t10
  br label %L5
L5:
  %t19 = load i64, ptr %t16
  %t20 = icmp eq i64 %t19, 43
  br i1 %t20, label %L6, label %L7
L6:
  store i64 1, ptr %t10
  br label %L7
L7:
  %t21 = load i64, ptr %t10
  %t22 = load ptr, ptr %t0
  %t23 = getelementptr i8, ptr %t22, i64 %t21
  %t24 = load i8, ptr %t23
  %t25 = zext i8 %t24 to i64
  %t26 = alloca i64
  store i64 %t25, ptr %t26
  %t27 = load i64, ptr %t26
  %t28 = icmp eq i64 %t27, 48
  br i1 %t28, label %L8, label %L9
L8:
  %t29 = load i64, ptr %t10
  %t30 = add i64 %t29, 1
  %t31 = load ptr, ptr %t0
  %t32 = getelementptr i8, ptr %t31, i64 %t30
  %t33 = load i8, ptr %t32
  %t34 = zext i8 %t33 to i64
  %t35 = alloca i64
  store i64 %t34, ptr %t35
  %t36 = load i64, ptr %t1
  %t37 = icmp eq i64 %t36, 16
  br i1 %t37, label %L10, label %L11
L10:
  %t38 = load i64, ptr %t35
  %t39 = icmp eq i64 %t38, 120
  br i1 %t39, label %L12, label %L13
L12:
  %t40 = load i64, ptr %t10
  %t41 = add i64 %t40, 2
  store i64 %t41, ptr %t10
  br label %L13
L13:
  %t42 = load i64, ptr %t35
  %t43 = icmp eq i64 %t42, 88
  br i1 %t43, label %L14, label %L15
L14:
  %t44 = load i64, ptr %t10
  %t45 = add i64 %t44, 2
  store i64 %t45, ptr %t10
  br label %L15
L15:
  br label %L11
L11:
  %t46 = load i64, ptr %t1
  %t47 = icmp eq i64 %t46, 8
  br i1 %t47, label %L16, label %L17
L16:
  %t48 = load i64, ptr %t35
  %t49 = icmp eq i64 %t48, 111
  br i1 %t49, label %L18, label %L19
L18:
  %t50 = load i64, ptr %t10
  %t51 = add i64 %t50, 2
  store i64 %t51, ptr %t10
  br label %L19
L19:
  %t52 = load i64, ptr %t35
  %t53 = icmp eq i64 %t52, 79
  br i1 %t53, label %L20, label %L21
L20:
  %t54 = load i64, ptr %t10
  %t55 = add i64 %t54, 2
  store i64 %t55, ptr %t10
  br label %L21
L21:
  br label %L17
L17:
  %t56 = load i64, ptr %t1
  %t57 = icmp eq i64 %t56, 2
  br i1 %t57, label %L22, label %L23
L22:
  %t58 = load i64, ptr %t35
  %t59 = icmp eq i64 %t58, 98
  br i1 %t59, label %L24, label %L25
L24:
  %t60 = load i64, ptr %t10
  %t61 = add i64 %t60, 2
  store i64 %t61, ptr %t10
  br label %L25
L25:
  %t62 = load i64, ptr %t35
  %t63 = icmp eq i64 %t62, 66
  br i1 %t63, label %L26, label %L27
L26:
  %t64 = load i64, ptr %t10
  %t65 = add i64 %t64, 2
  store i64 %t65, ptr %t10
  br label %L27
L27:
  br label %L23
L23:
  br label %L9
L9:
  %t66 = alloca i64
  store i64 0, ptr %t66
  %t67 = alloca i64
  store i64 0, ptr %t67
  br label %L28
L28:
  %t68 = load i64, ptr %t10
  %t69 = load ptr, ptr %t0
  %t70 = getelementptr i8, ptr %t69, i64 %t68
  %t71 = load i8, ptr %t70
  %t72 = trunc i64 0 to i8
  %t73 = icmp ne i8 %t71, %t72
  br i1 %t73, label %L29, label %L30
L29:
  %t74 = load i64, ptr %t10
  %t75 = load ptr, ptr %t0
  %t76 = getelementptr i8, ptr %t75, i64 %t74
  %t77 = load i8, ptr %t76
  %t78 = zext i8 %t77 to i64
  %t79 = alloca i64
  store i64 %t78, ptr %t79
  %t80 = load i64, ptr %t79
  %t81 = load i64, ptr %t1
  %t82 = call i64 @digit_val__string_2(i64 %t80, i64 %t81)
  %t83 = alloca i64
  store i64 %t82, ptr %t83
  %t84 = load i64, ptr %t83
  %t85 = icmp slt i64 %t84, 0
  br i1 %t85, label %L31, label %L32
L31:
  %t86 = insertvalue { i64, ptr } undef, i64 0, 0
  %t87 = insertvalue { i64, ptr } %t86, ptr @.str.5, 1
  ret { i64, ptr } %t87
L32:
  %t88 = load i64, ptr %t66
  %t89 = load i64, ptr %t83
  %t90 = sub i64 9223372036854775807, %t89
  %t91 = load i64, ptr %t1
  %t92 = sdiv i64 %t90, %t91
  %t93 = icmp sgt i64 %t88, %t92
  br i1 %t93, label %L33, label %L34
L33:
  %t94 = insertvalue { i64, ptr } undef, i64 0, 0
  %t95 = insertvalue { i64, ptr } %t94, ptr @.str.6, 1
  ret { i64, ptr } %t95
L34:
  %t96 = load i64, ptr %t66
  %t97 = load i64, ptr %t1
  %t98 = mul i64 %t96, %t97
  %t99 = load i64, ptr %t83
  %t100 = add i64 %t98, %t99
  store i64 %t100, ptr %t66
  %t101 = load i64, ptr %t67
  %t102 = add i64 %t101, 1
  store i64 %t102, ptr %t67
  %t103 = load i64, ptr %t10
  %t104 = add i64 %t103, 1
  store i64 %t104, ptr %t10
  br label %L28
L30:
  %t105 = load i64, ptr %t67
  %t106 = icmp eq i64 %t105, 0
  br i1 %t106, label %L35, label %L36
L35:
  %t107 = insertvalue { i64, ptr } undef, i64 0, 0
  %t108 = insertvalue { i64, ptr } %t107, ptr @.str.7, 1
  ret { i64, ptr } %t108
L36:
  %t109 = load i1, ptr %t11
  br i1 %t109, label %L37, label %L38
L37:
  %t110 = load i64, ptr %t66
  %t111 = sub i64 0, %t110
  store i64 %t111, ptr %t66
  br label %L38
L38:
  %t112 = load i64, ptr %t66
  %t113 = insertvalue { i64, ptr } undef, i64 %t112, 0
  %t114 = insertvalue { i64, ptr } %t113, ptr null, 1
  ret { i64, ptr } %t114
}

define { i64, ptr } @parse_int(ptr %a0) {
entry:
  %t0 = alloca ptr
  store ptr %a0, ptr %t0
  %t1 = load ptr, ptr %t0
  %t2 = call { i64, ptr } @parse_int_radix(ptr %t1, i64 10)
  ret { i64, ptr } %t2
}

define %StringBuilder @sb_new() {
entry:
  %t0 = call ptr @cool_gen_alloc(i64 8)
  %t1 = alloca ptr
  store ptr %t0, ptr %t1
  %t2 = load ptr, ptr %t1
  %t3 = getelementptr i8, ptr %t2, i64 0
  %t4 = trunc i64 0 to i8
  store i8 %t4, ptr %t3
  %t5 = load ptr, ptr %t1
  %t6 = insertvalue %StringBuilder undef, ptr %t5, 0
  %t7 = insertvalue %StringBuilder %t6, i64 0, 1
  %t8 = insertvalue %StringBuilder %t7, i64 8, 2
  ret %StringBuilder %t8
}

define void @sb_push_char({ ptr, i64 } %a0, i8 %a1) {
entry:
  %t0 = alloca { ptr, i64 }
  store { ptr, i64 } %a0, ptr %t0
  %t1 = alloca i8
  store i8 %a1, ptr %t1
  %t2 = load { ptr, i64 }, ptr %t0
  %t3 = extractvalue { ptr, i64 } %t2, 0
  %t4 = extractvalue { ptr, i64 } %t2, 1
  %t5 = icmp eq i64 %t4, 0
  br i1 %t5, label %L2, label %L0
L2:
  %t6 = icmp eq ptr %t3, null
  br i1 %t6, label %L3, label %L1
L3:
  call void @cool_null_fault()
  br label %L1
L0:
  %t7 = getelementptr i8, ptr %t3, i64 -8
  %t8 = load atomic i64, ptr %t7 seq_cst, align 8
  %t9 = icmp ne i64 %t8, %t4
  br i1 %t9, label %L4, label %L1
L4:
  call void @cool_gen_fault()
  br label %L1
L1:
  %t10 = getelementptr %StringBuilder, ptr %t3, i32 0, i32 1
  %t11 = load i64, ptr %t10
  %t12 = add i64 %t11, 1
  %t13 = load { ptr, i64 }, ptr %t0
  %t14 = extractvalue { ptr, i64 } %t13, 0
  %t15 = extractvalue { ptr, i64 } %t13, 1
  %t16 = icmp eq i64 %t15, 0
  br i1 %t16, label %L7, label %L5
L7:
  %t17 = icmp eq ptr %t14, null
  br i1 %t17, label %L8, label %L6
L8:
  call void @cool_null_fault()
  br label %L6
L5:
  %t18 = getelementptr i8, ptr %t14, i64 -8
  %t19 = load atomic i64, ptr %t18 seq_cst, align 8
  %t20 = icmp ne i64 %t19, %t15
  br i1 %t20, label %L9, label %L6
L9:
  call void @cool_gen_fault()
  br label %L6
L6:
  %t21 = getelementptr %StringBuilder, ptr %t14, i32 0, i32 2
  %t22 = load i64, ptr %t21
  %t23 = icmp sge i64 %t12, %t22
  br i1 %t23, label %L10, label %L11
L10:
  %t24 = load { ptr, i64 }, ptr %t0
  %t25 = extractvalue { ptr, i64 } %t24, 0
  %t26 = extractvalue { ptr, i64 } %t24, 1
  %t27 = icmp eq i64 %t26, 0
  br i1 %t27, label %L14, label %L12
L14:
  %t28 = icmp eq ptr %t25, null
  br i1 %t28, label %L15, label %L13
L15:
  call void @cool_null_fault()
  br label %L13
L12:
  %t29 = getelementptr i8, ptr %t25, i64 -8
  %t30 = load atomic i64, ptr %t29 seq_cst, align 8
  %t31 = icmp ne i64 %t30, %t26
  br i1 %t31, label %L16, label %L13
L16:
  call void @cool_gen_fault()
  br label %L13
L13:
  %t32 = getelementptr %StringBuilder, ptr %t25, i32 0, i32 2
  %t33 = load i64, ptr %t32
  %t34 = mul i64 %t33, 2
  %t35 = alloca i64
  store i64 %t34, ptr %t35
  %t36 = load i64, ptr %t35
  %t37 = icmp slt i64 %t36, 8
  br i1 %t37, label %L17, label %L18
L17:
  store i64 8, ptr %t35
  br label %L18
L18:
  %t38 = load i64, ptr %t35
  %t39 = call ptr @cool_gen_alloc(i64 %t38)
  %t40 = alloca ptr
  store ptr %t39, ptr %t40
  %t41 = alloca i64
  store i64 0, ptr %t41
  br label %L19
L19:
  %t42 = load i64, ptr %t41
  %t43 = load { ptr, i64 }, ptr %t0
  %t44 = extractvalue { ptr, i64 } %t43, 0
  %t45 = extractvalue { ptr, i64 } %t43, 1
  %t46 = icmp eq i64 %t45, 0
  br i1 %t46, label %L24, label %L22
L24:
  %t47 = icmp eq ptr %t44, null
  br i1 %t47, label %L25, label %L23
L25:
  call void @cool_null_fault()
  br label %L23
L22:
  %t48 = getelementptr i8, ptr %t44, i64 -8
  %t49 = load atomic i64, ptr %t48 seq_cst, align 8
  %t50 = icmp ne i64 %t49, %t45
  br i1 %t50, label %L26, label %L23
L26:
  call void @cool_gen_fault()
  br label %L23
L23:
  %t51 = getelementptr %StringBuilder, ptr %t44, i32 0, i32 1
  %t52 = load i64, ptr %t51
  %t53 = icmp slt i64 %t42, %t52
  br i1 %t53, label %L20, label %L21
L20:
  %t54 = load i64, ptr %t41
  %t55 = load ptr, ptr %t40
  %t56 = getelementptr i8, ptr %t55, i64 %t54
  %t57 = load i64, ptr %t41
  %t58 = load { ptr, i64 }, ptr %t0
  %t59 = extractvalue { ptr, i64 } %t58, 0
  %t60 = extractvalue { ptr, i64 } %t58, 1
  %t61 = icmp eq i64 %t60, 0
  br i1 %t61, label %L29, label %L27
L29:
  %t62 = icmp eq ptr %t59, null
  br i1 %t62, label %L30, label %L28
L30:
  call void @cool_null_fault()
  br label %L28
L27:
  %t63 = getelementptr i8, ptr %t59, i64 -8
  %t64 = load atomic i64, ptr %t63 seq_cst, align 8
  %t65 = icmp ne i64 %t64, %t60
  br i1 %t65, label %L31, label %L28
L31:
  call void @cool_gen_fault()
  br label %L28
L28:
  %t66 = getelementptr %StringBuilder, ptr %t59, i32 0, i32 0
  %t67 = load ptr, ptr %t66
  %t68 = getelementptr i8, ptr %t67, i64 %t57
  %t69 = load i8, ptr %t68
  store i8 %t69, ptr %t56
  %t70 = load i64, ptr %t41
  %t71 = add i64 %t70, 1
  store i64 %t71, ptr %t41
  br label %L19
L21:
  %t72 = load { ptr, i64 }, ptr %t0
  %t73 = extractvalue { ptr, i64 } %t72, 0
  %t74 = extractvalue { ptr, i64 } %t72, 1
  %t75 = icmp eq i64 %t74, 0
  br i1 %t75, label %L34, label %L32
L34:
  %t76 = icmp eq ptr %t73, null
  br i1 %t76, label %L35, label %L33
L35:
  call void @cool_null_fault()
  br label %L33
L32:
  %t77 = getelementptr i8, ptr %t73, i64 -8
  %t78 = load atomic i64, ptr %t77 seq_cst, align 8
  %t79 = icmp ne i64 %t78, %t74
  br i1 %t79, label %L36, label %L33
L36:
  call void @cool_gen_fault()
  br label %L33
L33:
  %t80 = getelementptr %StringBuilder, ptr %t73, i32 0, i32 0
  %t81 = load ptr, ptr %t80
  call void @cool_gen_free(ptr %t81)
  %t82 = load { ptr, i64 }, ptr %t0
  %t83 = extractvalue { ptr, i64 } %t82, 0
  %t84 = extractvalue { ptr, i64 } %t82, 1
  %t85 = icmp eq i64 %t84, 0
  br i1 %t85, label %L39, label %L37
L39:
  %t86 = icmp eq ptr %t83, null
  br i1 %t86, label %L40, label %L38
L40:
  call void @cool_null_fault()
  br label %L38
L37:
  %t87 = getelementptr i8, ptr %t83, i64 -8
  %t88 = load atomic i64, ptr %t87 seq_cst, align 8
  %t89 = icmp ne i64 %t88, %t84
  br i1 %t89, label %L41, label %L38
L41:
  call void @cool_gen_fault()
  br label %L38
L38:
  %t90 = getelementptr %StringBuilder, ptr %t83, i32 0, i32 0
  %t91 = load ptr, ptr %t40
  store ptr %t91, ptr %t90
  %t92 = load { ptr, i64 }, ptr %t0
  %t93 = extractvalue { ptr, i64 } %t92, 0
  %t94 = extractvalue { ptr, i64 } %t92, 1
  %t95 = icmp eq i64 %t94, 0
  br i1 %t95, label %L44, label %L42
L44:
  %t96 = icmp eq ptr %t93, null
  br i1 %t96, label %L45, label %L43
L45:
  call void @cool_null_fault()
  br label %L43
L42:
  %t97 = getelementptr i8, ptr %t93, i64 -8
  %t98 = load atomic i64, ptr %t97 seq_cst, align 8
  %t99 = icmp ne i64 %t98, %t94
  br i1 %t99, label %L46, label %L43
L46:
  call void @cool_gen_fault()
  br label %L43
L43:
  %t100 = getelementptr %StringBuilder, ptr %t93, i32 0, i32 2
  %t101 = load i64, ptr %t35
  store i64 %t101, ptr %t100
  br label %L11
L11:
  %t102 = load { ptr, i64 }, ptr %t0
  %t103 = extractvalue { ptr, i64 } %t102, 0
  %t104 = extractvalue { ptr, i64 } %t102, 1
  %t105 = icmp eq i64 %t104, 0
  br i1 %t105, label %L49, label %L47
L49:
  %t106 = icmp eq ptr %t103, null
  br i1 %t106, label %L50, label %L48
L50:
  call void @cool_null_fault()
  br label %L48
L47:
  %t107 = getelementptr i8, ptr %t103, i64 -8
  %t108 = load atomic i64, ptr %t107 seq_cst, align 8
  %t109 = icmp ne i64 %t108, %t104
  br i1 %t109, label %L51, label %L48
L51:
  call void @cool_gen_fault()
  br label %L48
L48:
  %t110 = getelementptr %StringBuilder, ptr %t103, i32 0, i32 1
  %t111 = load i64, ptr %t110
  %t112 = load { ptr, i64 }, ptr %t0
  %t113 = extractvalue { ptr, i64 } %t112, 0
  %t114 = extractvalue { ptr, i64 } %t112, 1
  %t115 = icmp eq i64 %t114, 0
  br i1 %t115, label %L54, label %L52
L54:
  %t116 = icmp eq ptr %t113, null
  br i1 %t116, label %L55, label %L53
L55:
  call void @cool_null_fault()
  br label %L53
L52:
  %t117 = getelementptr i8, ptr %t113, i64 -8
  %t118 = load atomic i64, ptr %t117 seq_cst, align 8
  %t119 = icmp ne i64 %t118, %t114
  br i1 %t119, label %L56, label %L53
L56:
  call void @cool_gen_fault()
  br label %L53
L53:
  %t120 = getelementptr %StringBuilder, ptr %t113, i32 0, i32 0
  %t121 = load ptr, ptr %t120
  %t122 = getelementptr i8, ptr %t121, i64 %t111
  %t123 = load i8, ptr %t1
  store i8 %t123, ptr %t122
  %t124 = load { ptr, i64 }, ptr %t0
  %t125 = extractvalue { ptr, i64 } %t124, 0
  %t126 = extractvalue { ptr, i64 } %t124, 1
  %t127 = icmp eq i64 %t126, 0
  br i1 %t127, label %L59, label %L57
L59:
  %t128 = icmp eq ptr %t125, null
  br i1 %t128, label %L60, label %L58
L60:
  call void @cool_null_fault()
  br label %L58
L57:
  %t129 = getelementptr i8, ptr %t125, i64 -8
  %t130 = load atomic i64, ptr %t129 seq_cst, align 8
  %t131 = icmp ne i64 %t130, %t126
  br i1 %t131, label %L61, label %L58
L61:
  call void @cool_gen_fault()
  br label %L58
L58:
  %t132 = getelementptr %StringBuilder, ptr %t125, i32 0, i32 1
  %t133 = load { ptr, i64 }, ptr %t0
  %t134 = extractvalue { ptr, i64 } %t133, 0
  %t135 = extractvalue { ptr, i64 } %t133, 1
  %t136 = icmp eq i64 %t135, 0
  br i1 %t136, label %L64, label %L62
L64:
  %t137 = icmp eq ptr %t134, null
  br i1 %t137, label %L65, label %L63
L65:
  call void @cool_null_fault()
  br label %L63
L62:
  %t138 = getelementptr i8, ptr %t134, i64 -8
  %t139 = load atomic i64, ptr %t138 seq_cst, align 8
  %t140 = icmp ne i64 %t139, %t135
  br i1 %t140, label %L66, label %L63
L66:
  call void @cool_gen_fault()
  br label %L63
L63:
  %t141 = getelementptr %StringBuilder, ptr %t134, i32 0, i32 1
  %t142 = load i64, ptr %t141
  %t143 = add i64 %t142, 1
  store i64 %t143, ptr %t132
  %t144 = load { ptr, i64 }, ptr %t0
  %t145 = extractvalue { ptr, i64 } %t144, 0
  %t146 = extractvalue { ptr, i64 } %t144, 1
  %t147 = icmp eq i64 %t146, 0
  br i1 %t147, label %L69, label %L67
L69:
  %t148 = icmp eq ptr %t145, null
  br i1 %t148, label %L70, label %L68
L70:
  call void @cool_null_fault()
  br label %L68
L67:
  %t149 = getelementptr i8, ptr %t145, i64 -8
  %t150 = load atomic i64, ptr %t149 seq_cst, align 8
  %t151 = icmp ne i64 %t150, %t146
  br i1 %t151, label %L71, label %L68
L71:
  call void @cool_gen_fault()
  br label %L68
L68:
  %t152 = getelementptr %StringBuilder, ptr %t145, i32 0, i32 1
  %t153 = load i64, ptr %t152
  %t154 = load { ptr, i64 }, ptr %t0
  %t155 = extractvalue { ptr, i64 } %t154, 0
  %t156 = extractvalue { ptr, i64 } %t154, 1
  %t157 = icmp eq i64 %t156, 0
  br i1 %t157, label %L74, label %L72
L74:
  %t158 = icmp eq ptr %t155, null
  br i1 %t158, label %L75, label %L73
L75:
  call void @cool_null_fault()
  br label %L73
L72:
  %t159 = getelementptr i8, ptr %t155, i64 -8
  %t160 = load atomic i64, ptr %t159 seq_cst, align 8
  %t161 = icmp ne i64 %t160, %t156
  br i1 %t161, label %L76, label %L73
L76:
  call void @cool_gen_fault()
  br label %L73
L73:
  %t162 = getelementptr %StringBuilder, ptr %t155, i32 0, i32 0
  %t163 = load ptr, ptr %t162
  %t164 = getelementptr i8, ptr %t163, i64 %t153
  %t165 = trunc i64 0 to i8
  store i8 %t165, ptr %t164
  ret void
}

define void @sb_push({ ptr, i64 } %a0, ptr %a1) {
entry:
  %t0 = alloca { ptr, i64 }
  store { ptr, i64 } %a0, ptr %t0
  %t1 = alloca ptr
  store ptr %a1, ptr %t1
  %t2 = alloca i64
  store i64 0, ptr %t2
  br label %L0
L0:
  %t3 = load i64, ptr %t2
  %t4 = load ptr, ptr %t1
  %t5 = getelementptr i8, ptr %t4, i64 %t3
  %t6 = load i8, ptr %t5
  %t7 = trunc i64 0 to i8
  %t8 = icmp ne i8 %t6, %t7
  br i1 %t8, label %L1, label %L2
L1:
  %t9 = load { ptr, i64 }, ptr %t0
  %t10 = load i64, ptr %t2
  %t11 = load ptr, ptr %t1
  %t12 = getelementptr i8, ptr %t11, i64 %t10
  %t13 = load i8, ptr %t12
  call void @sb_push_char({ ptr, i64 } %t9, i8 %t13)
  %t14 = load i64, ptr %t2
  %t15 = add i64 %t14, 1
  store i64 %t15, ptr %t2
  br label %L0
L2:
  ret void
}

define i64 @sb_size({ ptr, i64 } %a0) {
entry:
  %t0 = alloca { ptr, i64 }
  store { ptr, i64 } %a0, ptr %t0
  %t1 = load { ptr, i64 }, ptr %t0
  %t2 = extractvalue { ptr, i64 } %t1, 0
  %t3 = extractvalue { ptr, i64 } %t1, 1
  %t4 = icmp eq i64 %t3, 0
  br i1 %t4, label %L2, label %L0
L2:
  %t5 = icmp eq ptr %t2, null
  br i1 %t5, label %L3, label %L1
L3:
  call void @cool_null_fault()
  br label %L1
L0:
  %t6 = getelementptr i8, ptr %t2, i64 -8
  %t7 = load atomic i64, ptr %t6 seq_cst, align 8
  %t8 = icmp ne i64 %t7, %t3
  br i1 %t8, label %L4, label %L1
L4:
  call void @cool_gen_fault()
  br label %L1
L1:
  %t9 = getelementptr %StringBuilder, ptr %t2, i32 0, i32 1
  %t10 = load i64, ptr %t9
  ret i64 %t10
}

define ptr @sb_cstr({ ptr, i64 } %a0) {
entry:
  %t0 = alloca { ptr, i64 }
  store { ptr, i64 } %a0, ptr %t0
  %t1 = load { ptr, i64 }, ptr %t0
  %t2 = extractvalue { ptr, i64 } %t1, 0
  %t3 = extractvalue { ptr, i64 } %t1, 1
  %t4 = icmp eq i64 %t3, 0
  br i1 %t4, label %L2, label %L0
L2:
  %t5 = icmp eq ptr %t2, null
  br i1 %t5, label %L3, label %L1
L3:
  call void @cool_null_fault()
  br label %L1
L0:
  %t6 = getelementptr i8, ptr %t2, i64 -8
  %t7 = load atomic i64, ptr %t6 seq_cst, align 8
  %t8 = icmp ne i64 %t7, %t3
  br i1 %t8, label %L4, label %L1
L4:
  call void @cool_gen_fault()
  br label %L1
L1:
  %t9 = getelementptr %StringBuilder, ptr %t2, i32 0, i32 0
  %t10 = load ptr, ptr %t9
  ret ptr %t10
}

define void @sb_free({ ptr, i64 } %a0) {
entry:
  %t0 = alloca { ptr, i64 }
  store { ptr, i64 } %a0, ptr %t0
  %t1 = load { ptr, i64 }, ptr %t0
  %t2 = extractvalue { ptr, i64 } %t1, 0
  %t3 = extractvalue { ptr, i64 } %t1, 1
  %t4 = icmp eq i64 %t3, 0
  br i1 %t4, label %L2, label %L0
L2:
  %t5 = icmp eq ptr %t2, null
  br i1 %t5, label %L3, label %L1
L3:
  call void @cool_null_fault()
  br label %L1
L0:
  %t6 = getelementptr i8, ptr %t2, i64 -8
  %t7 = load atomic i64, ptr %t6 seq_cst, align 8
  %t8 = icmp ne i64 %t7, %t3
  br i1 %t8, label %L4, label %L1
L4:
  call void @cool_gen_fault()
  br label %L1
L1:
  %t9 = getelementptr %StringBuilder, ptr %t2, i32 0, i32 0
  %t10 = load ptr, ptr %t9
  call void @cool_gen_free(ptr %t10)
  ret void
}

define { ptr, i64 } @concat(ptr %a0, ptr %a1) {
entry:
  %t0 = alloca ptr
  store ptr %a0, ptr %t0
  %t1 = alloca ptr
  store ptr %a1, ptr %t1
  %t2 = call %StringBuilder @sb_new()
  %t3 = getelementptr %StringBuilder, ptr null, i64 1
  %t4 = ptrtoint ptr %t3 to i64
  %t5 = call ptr @cool_gen_alloc(i64 %t4)
  %t6 = getelementptr i8, ptr %t5, i64 -8
  %t7 = load atomic i64, ptr %t6 seq_cst, align 8
  store %StringBuilder %t2, ptr %t5
  %t8 = insertvalue { ptr, i64 } undef, ptr %t5, 0
  %t9 = insertvalue { ptr, i64 } %t8, i64 %t7, 1
  %t10 = alloca { ptr, i64 }
  store { ptr, i64 } %t9, ptr %t10
  %t11 = load { ptr, i64 }, ptr %t10
  %t12 = load ptr, ptr %t0
  call void @sb_push({ ptr, i64 } %t11, ptr %t12)
  %t13 = load { ptr, i64 }, ptr %t10
  %t14 = load ptr, ptr %t1
  call void @sb_push({ ptr, i64 } %t13, ptr %t14)
  %t15 = load { ptr, i64 }, ptr %t10
  ret { ptr, i64 } %t15
}

define ptr @loop_init() {
entry:
  %t0 = call i64 @cool_loop_init()
  %t1 = icmp eq i64 %t0, 1
  br i1 %t1, label %L0, label %L1
L0:
  ret ptr @.str.8
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
  ret ptr @.str.9
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

