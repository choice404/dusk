; ModuleID = 'dusk'
target triple = "x86_64-pc-linux-gnu"

%Pair = type { i32, i64 }
%Vector$Pair = type { ptr, i64, i64 }

@.str.0 = private unnamed_addr constant [2 x i8] c" \00"
@.str.1 = private unnamed_addr constant [24 x i8] c"vec_sort_stable.dusk:43\00"
@.str.2 = private unnamed_addr constant [34 x i8] c"fatal: vector index out of bounds\00"
@.str.3 = private unnamed_addr constant [2 x i8] c"\0A\00"
@.str.4 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:78\00"
@.str.5 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:51\00"
@.str.6 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:54\00"
@.str.7 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:74\00"
@.str.8 = private unnamed_addr constant [56 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:129\00"
@.str.9 = private unnamed_addr constant [56 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:152\00"
@.str.10 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:93\00"
@.str.11 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:94\00"
@.str.12 = private unnamed_addr constant [56 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:106\00"
@.str.13 = private unnamed_addr constant [56 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:111\00"
@.str.14 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:28\00"
@.str.15 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:29\00"
@.str.16 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:35\00"
@.str.17 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:36\00"
@.str.18 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:39\00"
@.str.19 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:40\00"
@.str.20 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:41\00"
@.str.21 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:43\00"
@.str.22 = private unnamed_addr constant [55 x i8] c"/home/austin/projects/cool-lang/lib/std/vector.dusk:44\00"

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
declare void @abort()

define i32 @cmp_pair(%Pair %a0, %Pair %a1) {
entry:
  %t0 = alloca %Pair
  %t1 = alloca %Pair
  store %Pair %a0, ptr %t0
  store %Pair %a1, ptr %t1
  %t2 = getelementptr %Pair, ptr %t0, i32 0, i32 0
  %t3 = load i32, ptr %t2
  %t4 = getelementptr %Pair, ptr %t1, i32 0, i32 0
  %t5 = load i32, ptr %t4
  %t6 = sub i32 %t3, %t5
  ret i32 %t6
}

define i32 @cmp_pair.funcval(ptr %env, %Pair %a0, %Pair %a1) {
entry:
  %r = call i32 @cmp_pair(%Pair %a0, %Pair %a1)
  ret i32 %r
}

define i32 @main() {
entry:
  %t0 = alloca i8
  %t9 = alloca { ptr, i64 }
  %t45 = alloca i64
  %t53 = alloca %Pair
  call void @cool_gc_anchor(ptr %t0)
  %t1 = call %Vector$Pair @vec_new$Pair()
  %t2 = getelementptr %Vector$Pair, ptr null, i64 1
  %t3 = ptrtoint ptr %t2 to i64
  %t4 = call ptr @cool_gen_alloc(i64 %t3)
  %t5 = getelementptr i8, ptr %t4, i64 -8
  %t6 = load atomic i64, ptr %t5 seq_cst, align 8
  store %Vector$Pair %t1, ptr %t4
  %t7 = insertvalue { ptr, i64 } undef, ptr %t4, 0
  %t8 = insertvalue { ptr, i64 } %t7, i64 %t6, 1
  store { ptr, i64 } %t8, ptr %t9
  %t10 = load { ptr, i64 }, ptr %t9
  %t11 = trunc i64 3 to i32
  %t12 = insertvalue %Pair undef, i32 %t11, 0
  %t13 = insertvalue %Pair %t12, i64 0, 1
  call void @vec_push$Pair({ ptr, i64 } %t10, %Pair %t13)
  %t14 = load { ptr, i64 }, ptr %t9
  %t15 = trunc i64 1 to i32
  %t16 = insertvalue %Pair undef, i32 %t15, 0
  %t17 = insertvalue %Pair %t16, i64 1, 1
  call void @vec_push$Pair({ ptr, i64 } %t14, %Pair %t17)
  %t18 = load { ptr, i64 }, ptr %t9
  %t19 = trunc i64 2 to i32
  %t20 = insertvalue %Pair undef, i32 %t19, 0
  %t21 = insertvalue %Pair %t20, i64 2, 1
  call void @vec_push$Pair({ ptr, i64 } %t18, %Pair %t21)
  %t22 = load { ptr, i64 }, ptr %t9
  %t23 = trunc i64 3 to i32
  %t24 = insertvalue %Pair undef, i32 %t23, 0
  %t25 = insertvalue %Pair %t24, i64 3, 1
  call void @vec_push$Pair({ ptr, i64 } %t22, %Pair %t25)
  %t26 = load { ptr, i64 }, ptr %t9
  %t27 = trunc i64 1 to i32
  %t28 = insertvalue %Pair undef, i32 %t27, 0
  %t29 = insertvalue %Pair %t28, i64 4, 1
  call void @vec_push$Pair({ ptr, i64 } %t26, %Pair %t29)
  %t30 = load { ptr, i64 }, ptr %t9
  %t31 = trunc i64 2 to i32
  %t32 = insertvalue %Pair undef, i32 %t31, 0
  %t33 = insertvalue %Pair %t32, i64 5, 1
  call void @vec_push$Pair({ ptr, i64 } %t30, %Pair %t33)
  %t34 = load { ptr, i64 }, ptr %t9
  %t35 = trunc i64 3 to i32
  %t36 = insertvalue %Pair undef, i32 %t35, 0
  %t37 = insertvalue %Pair %t36, i64 6, 1
  call void @vec_push$Pair({ ptr, i64 } %t34, %Pair %t37)
  %t38 = load { ptr, i64 }, ptr %t9
  %t39 = trunc i64 1 to i32
  %t40 = insertvalue %Pair undef, i32 %t39, 0
  %t41 = insertvalue %Pair %t40, i64 7, 1
  call void @vec_push$Pair({ ptr, i64 } %t38, %Pair %t41)
  %t42 = load { ptr, i64 }, ptr %t9
  %t43 = insertvalue { ptr, ptr } undef, ptr null, 0
  %t44 = insertvalue { ptr, ptr } %t43, ptr @cmp_pair.funcval, 1
  call void @vec_sort$Pair({ ptr, i64 } %t42, { ptr, ptr } %t44)
  store i64 0, ptr %t45
  br label %L0
L0:
  %t46 = load i64, ptr %t45
  %t47 = load { ptr, i64 }, ptr %t9
  %t48 = call i64 @vec_len$Pair({ ptr, i64 } %t47)
  %t49 = icmp slt i64 %t46, %t48
  br i1 %t49, label %L1, label %L2
L1:
  %t50 = load { ptr, i64 }, ptr %t9
  %t51 = load i64, ptr %t45
  %t52 = call %Pair @vec_get$Pair({ ptr, i64 } %t50, i64 %t51)
  store %Pair %t52, ptr %t53
  %t54 = getelementptr %Pair, ptr %t53, i32 0, i32 0
  %t55 = load i32, ptr %t54
  %t56 = sext i32 %t55 to i64
  call void @cool_print_i64(i64 %t56)
  call void @cool_print_cstr(ptr @.str.0)
  %t57 = getelementptr %Pair, ptr %t53, i32 0, i32 1
  %t58 = load i64, ptr %t57
  call void @cool_println_i64(i64 %t58)
  %t59 = load i64, ptr %t45
  %t60 = add i64 %t59, 1
  store i64 %t60, ptr %t45
  br label %L0
L2:
  %t61 = load { ptr, i64 }, ptr %t9
  call void @vec_free$Pair({ ptr, i64 } %t61)
  %t62 = load { ptr, i64 }, ptr %t9
  %t63 = extractvalue { ptr, i64 } %t62, 0
  %t64 = extractvalue { ptr, i64 } %t62, 1
  %t65 = icmp eq i64 %t64, 0
  br i1 %t65, label %L5, label %L3
L5:
  %t66 = icmp eq ptr %t63, null
  br i1 %t66, label %L6, label %L4
L6:
  call void @cool_null_fault_at(ptr @.str.1)
  br label %L4
L3:
  %t67 = getelementptr i8, ptr %t63, i64 -8
  %t68 = load atomic i64, ptr %t67 seq_cst, align 8
  %t69 = icmp ne i64 %t68, %t64
  br i1 %t69, label %L7, label %L4
L7:
  call void @cool_gen_fault_at(ptr @.str.1)
  br label %L4
L4:
  call void @cool_gen_free(ptr %t63)
  %t70 = trunc i64 0 to i32
  ret i32 %t70
}

define void @vec_bounds_fault__vector_1() {
entry:
  call void @cool_eprint_cstr(ptr @.str.2)
  call void @cool_eprint_cstr(ptr @.str.3)
  call void @abort()
  ret void
}

define void @vec_free$Pair({ ptr, i64 } %a0) {
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
  call void @cool_null_fault_at(ptr @.str.4)
  br label %L1
L0:
  %t6 = getelementptr i8, ptr %t2, i64 -8
  %t7 = load atomic i64, ptr %t6 seq_cst, align 8
  %t8 = icmp ne i64 %t7, %t3
  br i1 %t8, label %L4, label %L1
L4:
  call void @cool_gen_fault_at(ptr @.str.4)
  br label %L1
L1:
  %t9 = getelementptr %Vector$Pair, ptr %t2, i32 0, i32 0
  %t10 = load ptr, ptr %t9
  call void @cool_gen_free(ptr %t10)
  ret void
}

define %Pair @vec_get$Pair({ ptr, i64 } %a0, i64 %a1) {
entry:
  %t0 = alloca { ptr, i64 }
  %t1 = alloca i64
  store { ptr, i64 } %a0, ptr %t0
  store i64 %a1, ptr %t1
  %t2 = load i64, ptr %t1
  %t3 = icmp slt i64 %t2, 0
  br i1 %t3, label %L0, label %L1
L0:
  call void @vec_bounds_fault__vector_1()
  br label %L1
L1:
  %t4 = load i64, ptr %t1
  %t5 = load { ptr, i64 }, ptr %t0
  %t6 = extractvalue { ptr, i64 } %t5, 0
  %t7 = extractvalue { ptr, i64 } %t5, 1
  %t8 = icmp eq i64 %t7, 0
  br i1 %t8, label %L4, label %L2
L4:
  %t9 = icmp eq ptr %t6, null
  br i1 %t9, label %L5, label %L3
L5:
  call void @cool_null_fault_at(ptr @.str.5)
  br label %L3
L2:
  %t10 = getelementptr i8, ptr %t6, i64 -8
  %t11 = load atomic i64, ptr %t10 seq_cst, align 8
  %t12 = icmp ne i64 %t11, %t7
  br i1 %t12, label %L6, label %L3
L6:
  call void @cool_gen_fault_at(ptr @.str.5)
  br label %L3
L3:
  %t13 = getelementptr %Vector$Pair, ptr %t6, i32 0, i32 1
  %t14 = load i64, ptr %t13
  %t15 = icmp sge i64 %t4, %t14
  br i1 %t15, label %L7, label %L8
L7:
  call void @vec_bounds_fault__vector_1()
  br label %L8
L8:
  %t16 = load i64, ptr %t1
  %t17 = load { ptr, i64 }, ptr %t0
  %t18 = extractvalue { ptr, i64 } %t17, 0
  %t19 = extractvalue { ptr, i64 } %t17, 1
  %t20 = icmp eq i64 %t19, 0
  br i1 %t20, label %L11, label %L9
L11:
  %t21 = icmp eq ptr %t18, null
  br i1 %t21, label %L12, label %L10
L12:
  call void @cool_null_fault_at(ptr @.str.6)
  br label %L10
L9:
  %t22 = getelementptr i8, ptr %t18, i64 -8
  %t23 = load atomic i64, ptr %t22 seq_cst, align 8
  %t24 = icmp ne i64 %t23, %t19
  br i1 %t24, label %L13, label %L10
L13:
  call void @cool_gen_fault_at(ptr @.str.6)
  br label %L10
L10:
  %t25 = getelementptr %Vector$Pair, ptr %t18, i32 0, i32 0
  %t26 = load ptr, ptr %t25
  %t27 = getelementptr %Pair, ptr %t26, i64 %t16
  %t28 = load %Pair, ptr %t27
  ret %Pair %t28
}

define i64 @vec_len$Pair({ ptr, i64 } %a0) {
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
  call void @cool_null_fault_at(ptr @.str.7)
  br label %L1
L0:
  %t6 = getelementptr i8, ptr %t2, i64 -8
  %t7 = load atomic i64, ptr %t6 seq_cst, align 8
  %t8 = icmp ne i64 %t7, %t3
  br i1 %t8, label %L4, label %L1
L4:
  call void @cool_gen_fault_at(ptr @.str.7)
  br label %L1
L1:
  %t9 = getelementptr %Vector$Pair, ptr %t2, i32 0, i32 1
  %t10 = load i64, ptr %t9
  ret i64 %t10
}

define void @vec_sort$Pair({ ptr, i64 } %a0, { ptr, ptr } %a1) {
entry:
  %t0 = alloca { ptr, i64 }
  %t1 = alloca { ptr, ptr }
  %t12 = alloca i64
  %t20 = alloca ptr
  %t21 = alloca i64
  %t25 = alloca i64
  %t30 = alloca i64
  %t34 = alloca i64
  %t43 = alloca i64
  %t58 = alloca i64
  store { ptr, i64 } %a0, ptr %t0
  store { ptr, ptr } %a1, ptr %t1
  %t2 = load { ptr, i64 }, ptr %t0
  %t3 = extractvalue { ptr, i64 } %t2, 0
  %t4 = extractvalue { ptr, i64 } %t2, 1
  %t5 = icmp eq i64 %t4, 0
  br i1 %t5, label %L2, label %L0
L2:
  %t6 = icmp eq ptr %t3, null
  br i1 %t6, label %L3, label %L1
L3:
  call void @cool_null_fault_at(ptr @.str.8)
  br label %L1
L0:
  %t7 = getelementptr i8, ptr %t3, i64 -8
  %t8 = load atomic i64, ptr %t7 seq_cst, align 8
  %t9 = icmp ne i64 %t8, %t4
  br i1 %t9, label %L4, label %L1
L4:
  call void @cool_gen_fault_at(ptr @.str.8)
  br label %L1
L1:
  %t10 = getelementptr %Vector$Pair, ptr %t3, i32 0, i32 1
  %t11 = load i64, ptr %t10
  store i64 %t11, ptr %t12
  %t13 = load i64, ptr %t12
  %t14 = icmp slt i64 %t13, 2
  br i1 %t14, label %L5, label %L6
L5:
  ret void
L6:
  %t15 = load i64, ptr %t12
  %t16 = getelementptr %Pair, ptr null, i64 1
  %t17 = ptrtoint ptr %t16 to i64
  %t18 = mul i64 %t15, %t17
  %t19 = call ptr @cool_gen_alloc(i64 %t18)
  store ptr %t19, ptr %t20
  store i64 1, ptr %t21
  br label %L7
L7:
  %t22 = load i64, ptr %t21
  %t23 = load i64, ptr %t12
  %t24 = icmp slt i64 %t22, %t23
  br i1 %t24, label %L8, label %L9
L8:
  store i64 0, ptr %t25
  br label %L10
L10:
  %t26 = load i64, ptr %t25
  %t27 = load i64, ptr %t12
  %t28 = icmp slt i64 %t26, %t27
  br i1 %t28, label %L11, label %L12
L11:
  %t29 = load i64, ptr %t25
  store i64 %t29, ptr %t30
  %t31 = load i64, ptr %t25
  %t32 = load i64, ptr %t21
  %t33 = add i64 %t31, %t32
  store i64 %t33, ptr %t34
  %t35 = load i64, ptr %t34
  %t36 = load i64, ptr %t12
  %t37 = icmp sgt i64 %t35, %t36
  br i1 %t37, label %L13, label %L14
L13:
  %t38 = load i64, ptr %t12
  store i64 %t38, ptr %t34
  br label %L14
L14:
  %t39 = load i64, ptr %t25
  %t40 = load i64, ptr %t21
  %t41 = mul i64 2, %t40
  %t42 = add i64 %t39, %t41
  store i64 %t42, ptr %t43
  %t44 = load i64, ptr %t43
  %t45 = load i64, ptr %t12
  %t46 = icmp sgt i64 %t44, %t45
  br i1 %t46, label %L15, label %L16
L15:
  %t47 = load i64, ptr %t12
  store i64 %t47, ptr %t43
  br label %L16
L16:
  %t48 = load { ptr, i64 }, ptr %t0
  %t49 = load ptr, ptr %t20
  %t50 = load i64, ptr %t30
  %t51 = load i64, ptr %t34
  %t52 = load i64, ptr %t43
  %t53 = load { ptr, ptr }, ptr %t1
  call void @vec_sort_merge_run__vector_1$Pair({ ptr, i64 } %t48, ptr %t49, i64 %t50, i64 %t51, i64 %t52, { ptr, ptr } %t53)
  %t54 = load i64, ptr %t25
  %t55 = load i64, ptr %t21
  %t56 = mul i64 2, %t55
  %t57 = add i64 %t54, %t56
  store i64 %t57, ptr %t25
  br label %L10
L12:
  store i64 0, ptr %t58
  br label %L17
L17:
  %t59 = load i64, ptr %t58
  %t60 = load i64, ptr %t12
  %t61 = icmp slt i64 %t59, %t60
  br i1 %t61, label %L18, label %L19
L18:
  %t62 = load i64, ptr %t58
  %t63 = load { ptr, i64 }, ptr %t0
  %t64 = extractvalue { ptr, i64 } %t63, 0
  %t65 = extractvalue { ptr, i64 } %t63, 1
  %t66 = icmp eq i64 %t65, 0
  br i1 %t66, label %L22, label %L20
L22:
  %t67 = icmp eq ptr %t64, null
  br i1 %t67, label %L23, label %L21
L23:
  call void @cool_null_fault_at(ptr @.str.9)
  br label %L21
L20:
  %t68 = getelementptr i8, ptr %t64, i64 -8
  %t69 = load atomic i64, ptr %t68 seq_cst, align 8
  %t70 = icmp ne i64 %t69, %t65
  br i1 %t70, label %L24, label %L21
L24:
  call void @cool_gen_fault_at(ptr @.str.9)
  br label %L21
L21:
  %t71 = getelementptr %Vector$Pair, ptr %t64, i32 0, i32 0
  %t72 = load ptr, ptr %t71
  %t73 = getelementptr %Pair, ptr %t72, i64 %t62
  %t74 = load i64, ptr %t58
  %t75 = load ptr, ptr %t20
  %t76 = getelementptr %Pair, ptr %t75, i64 %t74
  %t77 = load %Pair, ptr %t76
  store %Pair %t77, ptr %t73
  %t78 = load i64, ptr %t58
  %t79 = add i64 %t78, 1
  store i64 %t79, ptr %t58
  br label %L17
L19:
  %t80 = load i64, ptr %t21
  %t81 = mul i64 %t80, 2
  store i64 %t81, ptr %t21
  br label %L7
L9:
  %t82 = load ptr, ptr %t20
  call void @cool_gen_free(ptr %t82)
  ret void
}

define void @vec_sort_merge_run__vector_1$Pair({ ptr, i64 } %a0, ptr %a1, i64 %a2, i64 %a3, i64 %a4, { ptr, ptr } %a5) {
entry:
  %t0 = alloca { ptr, i64 }
  %t1 = alloca ptr
  %t2 = alloca i64
  %t3 = alloca i64
  %t4 = alloca i64
  %t5 = alloca { ptr, ptr }
  %t7 = alloca i64
  %t9 = alloca i64
  %t11 = alloca i64
  %t15 = alloca i1
  %t33 = alloca %Pair
  %t47 = alloca %Pair
  %t54 = alloca i32
  store { ptr, i64 } %a0, ptr %t0
  store ptr %a1, ptr %t1
  store i64 %a2, ptr %t2
  store i64 %a3, ptr %t3
  store i64 %a4, ptr %t4
  store { ptr, ptr } %a5, ptr %t5
  %t6 = load i64, ptr %t2
  store i64 %t6, ptr %t7
  %t8 = load i64, ptr %t3
  store i64 %t8, ptr %t9
  %t10 = load i64, ptr %t2
  store i64 %t10, ptr %t11
  br label %L0
L0:
  %t12 = load i64, ptr %t7
  %t13 = load i64, ptr %t3
  %t14 = icmp slt i64 %t12, %t13
  store i1 %t14, ptr %t15
  br i1 %t14, label %L3, label %L4
L3:
  %t16 = load i64, ptr %t9
  %t17 = load i64, ptr %t4
  %t18 = icmp slt i64 %t16, %t17
  store i1 %t18, ptr %t15
  br label %L4
L4:
  %t19 = load i1, ptr %t15
  br i1 %t19, label %L1, label %L2
L1:
  %t20 = load i64, ptr %t7
  %t21 = load { ptr, i64 }, ptr %t0
  %t22 = extractvalue { ptr, i64 } %t21, 0
  %t23 = extractvalue { ptr, i64 } %t21, 1
  %t24 = icmp eq i64 %t23, 0
  br i1 %t24, label %L7, label %L5
L7:
  %t25 = icmp eq ptr %t22, null
  br i1 %t25, label %L8, label %L6
L8:
  call void @cool_null_fault_at(ptr @.str.10)
  br label %L6
L5:
  %t26 = getelementptr i8, ptr %t22, i64 -8
  %t27 = load atomic i64, ptr %t26 seq_cst, align 8
  %t28 = icmp ne i64 %t27, %t23
  br i1 %t28, label %L9, label %L6
L9:
  call void @cool_gen_fault_at(ptr @.str.10)
  br label %L6
L6:
  %t29 = getelementptr %Vector$Pair, ptr %t22, i32 0, i32 0
  %t30 = load ptr, ptr %t29
  %t31 = getelementptr %Pair, ptr %t30, i64 %t20
  %t32 = load %Pair, ptr %t31
  store %Pair %t32, ptr %t33
  %t34 = load i64, ptr %t9
  %t35 = load { ptr, i64 }, ptr %t0
  %t36 = extractvalue { ptr, i64 } %t35, 0
  %t37 = extractvalue { ptr, i64 } %t35, 1
  %t38 = icmp eq i64 %t37, 0
  br i1 %t38, label %L12, label %L10
L12:
  %t39 = icmp eq ptr %t36, null
  br i1 %t39, label %L13, label %L11
L13:
  call void @cool_null_fault_at(ptr @.str.11)
  br label %L11
L10:
  %t40 = getelementptr i8, ptr %t36, i64 -8
  %t41 = load atomic i64, ptr %t40 seq_cst, align 8
  %t42 = icmp ne i64 %t41, %t37
  br i1 %t42, label %L14, label %L11
L14:
  call void @cool_gen_fault_at(ptr @.str.11)
  br label %L11
L11:
  %t43 = getelementptr %Vector$Pair, ptr %t36, i32 0, i32 0
  %t44 = load ptr, ptr %t43
  %t45 = getelementptr %Pair, ptr %t44, i64 %t34
  %t46 = load %Pair, ptr %t45
  store %Pair %t46, ptr %t47
  %t48 = load { ptr, ptr }, ptr %t5
  %t49 = load %Pair, ptr %t33
  %t50 = load %Pair, ptr %t47
  %t51 = extractvalue { ptr, ptr } %t48, 0
  %t52 = extractvalue { ptr, ptr } %t48, 1
  %t53 = call i32 %t52(ptr %t51, %Pair %t49, %Pair %t50)
  store i32 %t53, ptr %t54
  %t55 = load i32, ptr %t54
  %t56 = trunc i64 0 to i32
  %t57 = icmp sle i32 %t55, %t56
  br i1 %t57, label %L15, label %L17
L15:
  %t58 = load i64, ptr %t11
  %t59 = load ptr, ptr %t1
  %t60 = getelementptr %Pair, ptr %t59, i64 %t58
  %t61 = load %Pair, ptr %t33
  store %Pair %t61, ptr %t60
  %t62 = load i64, ptr %t7
  %t63 = add i64 %t62, 1
  store i64 %t63, ptr %t7
  br label %L16
L17:
  %t64 = load i64, ptr %t11
  %t65 = load ptr, ptr %t1
  %t66 = getelementptr %Pair, ptr %t65, i64 %t64
  %t67 = load %Pair, ptr %t47
  store %Pair %t67, ptr %t66
  %t68 = load i64, ptr %t9
  %t69 = add i64 %t68, 1
  store i64 %t69, ptr %t9
  br label %L16
L16:
  %t70 = load i64, ptr %t11
  %t71 = add i64 %t70, 1
  store i64 %t71, ptr %t11
  br label %L0
L2:
  br label %L18
L18:
  %t72 = load i64, ptr %t7
  %t73 = load i64, ptr %t3
  %t74 = icmp slt i64 %t72, %t73
  br i1 %t74, label %L19, label %L20
L19:
  %t75 = load i64, ptr %t11
  %t76 = load ptr, ptr %t1
  %t77 = getelementptr %Pair, ptr %t76, i64 %t75
  %t78 = load i64, ptr %t7
  %t79 = load { ptr, i64 }, ptr %t0
  %t80 = extractvalue { ptr, i64 } %t79, 0
  %t81 = extractvalue { ptr, i64 } %t79, 1
  %t82 = icmp eq i64 %t81, 0
  br i1 %t82, label %L23, label %L21
L23:
  %t83 = icmp eq ptr %t80, null
  br i1 %t83, label %L24, label %L22
L24:
  call void @cool_null_fault_at(ptr @.str.12)
  br label %L22
L21:
  %t84 = getelementptr i8, ptr %t80, i64 -8
  %t85 = load atomic i64, ptr %t84 seq_cst, align 8
  %t86 = icmp ne i64 %t85, %t81
  br i1 %t86, label %L25, label %L22
L25:
  call void @cool_gen_fault_at(ptr @.str.12)
  br label %L22
L22:
  %t87 = getelementptr %Vector$Pair, ptr %t80, i32 0, i32 0
  %t88 = load ptr, ptr %t87
  %t89 = getelementptr %Pair, ptr %t88, i64 %t78
  %t90 = load %Pair, ptr %t89
  store %Pair %t90, ptr %t77
  %t91 = load i64, ptr %t7
  %t92 = add i64 %t91, 1
  store i64 %t92, ptr %t7
  %t93 = load i64, ptr %t11
  %t94 = add i64 %t93, 1
  store i64 %t94, ptr %t11
  br label %L18
L20:
  br label %L26
L26:
  %t95 = load i64, ptr %t9
  %t96 = load i64, ptr %t4
  %t97 = icmp slt i64 %t95, %t96
  br i1 %t97, label %L27, label %L28
L27:
  %t98 = load i64, ptr %t11
  %t99 = load ptr, ptr %t1
  %t100 = getelementptr %Pair, ptr %t99, i64 %t98
  %t101 = load i64, ptr %t9
  %t102 = load { ptr, i64 }, ptr %t0
  %t103 = extractvalue { ptr, i64 } %t102, 0
  %t104 = extractvalue { ptr, i64 } %t102, 1
  %t105 = icmp eq i64 %t104, 0
  br i1 %t105, label %L31, label %L29
L31:
  %t106 = icmp eq ptr %t103, null
  br i1 %t106, label %L32, label %L30
L32:
  call void @cool_null_fault_at(ptr @.str.13)
  br label %L30
L29:
  %t107 = getelementptr i8, ptr %t103, i64 -8
  %t108 = load atomic i64, ptr %t107 seq_cst, align 8
  %t109 = icmp ne i64 %t108, %t104
  br i1 %t109, label %L33, label %L30
L33:
  call void @cool_gen_fault_at(ptr @.str.13)
  br label %L30
L30:
  %t110 = getelementptr %Vector$Pair, ptr %t103, i32 0, i32 0
  %t111 = load ptr, ptr %t110
  %t112 = getelementptr %Pair, ptr %t111, i64 %t101
  %t113 = load %Pair, ptr %t112
  store %Pair %t113, ptr %t100
  %t114 = load i64, ptr %t9
  %t115 = add i64 %t114, 1
  store i64 %t115, ptr %t9
  %t116 = load i64, ptr %t11
  %t117 = add i64 %t116, 1
  store i64 %t117, ptr %t11
  br label %L26
L28:
  ret void
}

define void @vec_push$Pair({ ptr, i64 } %a0, %Pair %a1) {
entry:
  %t0 = alloca { ptr, i64 }
  %t1 = alloca %Pair
  %t34 = alloca i64
  %t42 = alloca ptr
  %t43 = alloca i64
  store { ptr, i64 } %a0, ptr %t0
  store %Pair %a1, ptr %t1
  %t2 = load { ptr, i64 }, ptr %t0
  %t3 = extractvalue { ptr, i64 } %t2, 0
  %t4 = extractvalue { ptr, i64 } %t2, 1
  %t5 = icmp eq i64 %t4, 0
  br i1 %t5, label %L2, label %L0
L2:
  %t6 = icmp eq ptr %t3, null
  br i1 %t6, label %L3, label %L1
L3:
  call void @cool_null_fault_at(ptr @.str.14)
  br label %L1
L0:
  %t7 = getelementptr i8, ptr %t3, i64 -8
  %t8 = load atomic i64, ptr %t7 seq_cst, align 8
  %t9 = icmp ne i64 %t8, %t4
  br i1 %t9, label %L4, label %L1
L4:
  call void @cool_gen_fault_at(ptr @.str.14)
  br label %L1
L1:
  %t10 = getelementptr %Vector$Pair, ptr %t3, i32 0, i32 1
  %t11 = load i64, ptr %t10
  %t12 = load { ptr, i64 }, ptr %t0
  %t13 = extractvalue { ptr, i64 } %t12, 0
  %t14 = extractvalue { ptr, i64 } %t12, 1
  %t15 = icmp eq i64 %t14, 0
  br i1 %t15, label %L7, label %L5
L7:
  %t16 = icmp eq ptr %t13, null
  br i1 %t16, label %L8, label %L6
L8:
  call void @cool_null_fault_at(ptr @.str.14)
  br label %L6
L5:
  %t17 = getelementptr i8, ptr %t13, i64 -8
  %t18 = load atomic i64, ptr %t17 seq_cst, align 8
  %t19 = icmp ne i64 %t18, %t14
  br i1 %t19, label %L9, label %L6
L9:
  call void @cool_gen_fault_at(ptr @.str.14)
  br label %L6
L6:
  %t20 = getelementptr %Vector$Pair, ptr %t13, i32 0, i32 2
  %t21 = load i64, ptr %t20
  %t22 = icmp eq i64 %t11, %t21
  br i1 %t22, label %L10, label %L11
L10:
  %t23 = load { ptr, i64 }, ptr %t0
  %t24 = extractvalue { ptr, i64 } %t23, 0
  %t25 = extractvalue { ptr, i64 } %t23, 1
  %t26 = icmp eq i64 %t25, 0
  br i1 %t26, label %L14, label %L12
L14:
  %t27 = icmp eq ptr %t24, null
  br i1 %t27, label %L15, label %L13
L15:
  call void @cool_null_fault_at(ptr @.str.15)
  br label %L13
L12:
  %t28 = getelementptr i8, ptr %t24, i64 -8
  %t29 = load atomic i64, ptr %t28 seq_cst, align 8
  %t30 = icmp ne i64 %t29, %t25
  br i1 %t30, label %L16, label %L13
L16:
  call void @cool_gen_fault_at(ptr @.str.15)
  br label %L13
L13:
  %t31 = getelementptr %Vector$Pair, ptr %t24, i32 0, i32 2
  %t32 = load i64, ptr %t31
  %t33 = mul i64 %t32, 2
  store i64 %t33, ptr %t34
  %t35 = load i64, ptr %t34
  %t36 = icmp eq i64 %t35, 0
  br i1 %t36, label %L17, label %L18
L17:
  store i64 4, ptr %t34
  br label %L18
L18:
  %t37 = load i64, ptr %t34
  %t38 = getelementptr %Pair, ptr null, i64 1
  %t39 = ptrtoint ptr %t38 to i64
  %t40 = mul i64 %t37, %t39
  %t41 = call ptr @cool_gen_alloc(i64 %t40)
  store ptr %t41, ptr %t42
  store i64 0, ptr %t43
  br label %L19
L19:
  %t44 = load i64, ptr %t43
  %t45 = load { ptr, i64 }, ptr %t0
  %t46 = extractvalue { ptr, i64 } %t45, 0
  %t47 = extractvalue { ptr, i64 } %t45, 1
  %t48 = icmp eq i64 %t47, 0
  br i1 %t48, label %L24, label %L22
L24:
  %t49 = icmp eq ptr %t46, null
  br i1 %t49, label %L25, label %L23
L25:
  call void @cool_null_fault_at(ptr @.str.16)
  br label %L23
L22:
  %t50 = getelementptr i8, ptr %t46, i64 -8
  %t51 = load atomic i64, ptr %t50 seq_cst, align 8
  %t52 = icmp ne i64 %t51, %t47
  br i1 %t52, label %L26, label %L23
L26:
  call void @cool_gen_fault_at(ptr @.str.16)
  br label %L23
L23:
  %t53 = getelementptr %Vector$Pair, ptr %t46, i32 0, i32 1
  %t54 = load i64, ptr %t53
  %t55 = icmp slt i64 %t44, %t54
  br i1 %t55, label %L20, label %L21
L20:
  %t56 = load i64, ptr %t43
  %t57 = load ptr, ptr %t42
  %t58 = getelementptr %Pair, ptr %t57, i64 %t56
  %t59 = load i64, ptr %t43
  %t60 = load { ptr, i64 }, ptr %t0
  %t61 = extractvalue { ptr, i64 } %t60, 0
  %t62 = extractvalue { ptr, i64 } %t60, 1
  %t63 = icmp eq i64 %t62, 0
  br i1 %t63, label %L29, label %L27
L29:
  %t64 = icmp eq ptr %t61, null
  br i1 %t64, label %L30, label %L28
L30:
  call void @cool_null_fault_at(ptr @.str.17)
  br label %L28
L27:
  %t65 = getelementptr i8, ptr %t61, i64 -8
  %t66 = load atomic i64, ptr %t65 seq_cst, align 8
  %t67 = icmp ne i64 %t66, %t62
  br i1 %t67, label %L31, label %L28
L31:
  call void @cool_gen_fault_at(ptr @.str.17)
  br label %L28
L28:
  %t68 = getelementptr %Vector$Pair, ptr %t61, i32 0, i32 0
  %t69 = load ptr, ptr %t68
  %t70 = getelementptr %Pair, ptr %t69, i64 %t59
  %t71 = load %Pair, ptr %t70
  store %Pair %t71, ptr %t58
  %t72 = load i64, ptr %t43
  %t73 = add i64 %t72, 1
  store i64 %t73, ptr %t43
  br label %L19
L21:
  %t74 = load { ptr, i64 }, ptr %t0
  %t75 = extractvalue { ptr, i64 } %t74, 0
  %t76 = extractvalue { ptr, i64 } %t74, 1
  %t77 = icmp eq i64 %t76, 0
  br i1 %t77, label %L34, label %L32
L34:
  %t78 = icmp eq ptr %t75, null
  br i1 %t78, label %L35, label %L33
L35:
  call void @cool_null_fault_at(ptr @.str.18)
  br label %L33
L32:
  %t79 = getelementptr i8, ptr %t75, i64 -8
  %t80 = load atomic i64, ptr %t79 seq_cst, align 8
  %t81 = icmp ne i64 %t80, %t76
  br i1 %t81, label %L36, label %L33
L36:
  call void @cool_gen_fault_at(ptr @.str.18)
  br label %L33
L33:
  %t82 = getelementptr %Vector$Pair, ptr %t75, i32 0, i32 0
  %t83 = load ptr, ptr %t82
  call void @cool_gen_free(ptr %t83)
  %t84 = load { ptr, i64 }, ptr %t0
  %t85 = extractvalue { ptr, i64 } %t84, 0
  %t86 = extractvalue { ptr, i64 } %t84, 1
  %t87 = icmp eq i64 %t86, 0
  br i1 %t87, label %L39, label %L37
L39:
  %t88 = icmp eq ptr %t85, null
  br i1 %t88, label %L40, label %L38
L40:
  call void @cool_null_fault_at(ptr @.str.19)
  br label %L38
L37:
  %t89 = getelementptr i8, ptr %t85, i64 -8
  %t90 = load atomic i64, ptr %t89 seq_cst, align 8
  %t91 = icmp ne i64 %t90, %t86
  br i1 %t91, label %L41, label %L38
L41:
  call void @cool_gen_fault_at(ptr @.str.19)
  br label %L38
L38:
  %t92 = getelementptr %Vector$Pair, ptr %t85, i32 0, i32 0
  %t93 = load ptr, ptr %t42
  store ptr %t93, ptr %t92
  %t94 = load { ptr, i64 }, ptr %t0
  %t95 = extractvalue { ptr, i64 } %t94, 0
  %t96 = extractvalue { ptr, i64 } %t94, 1
  %t97 = icmp eq i64 %t96, 0
  br i1 %t97, label %L44, label %L42
L44:
  %t98 = icmp eq ptr %t95, null
  br i1 %t98, label %L45, label %L43
L45:
  call void @cool_null_fault_at(ptr @.str.20)
  br label %L43
L42:
  %t99 = getelementptr i8, ptr %t95, i64 -8
  %t100 = load atomic i64, ptr %t99 seq_cst, align 8
  %t101 = icmp ne i64 %t100, %t96
  br i1 %t101, label %L46, label %L43
L46:
  call void @cool_gen_fault_at(ptr @.str.20)
  br label %L43
L43:
  %t102 = getelementptr %Vector$Pair, ptr %t95, i32 0, i32 2
  %t103 = load i64, ptr %t34
  store i64 %t103, ptr %t102
  br label %L11
L11:
  %t104 = load { ptr, i64 }, ptr %t0
  %t105 = extractvalue { ptr, i64 } %t104, 0
  %t106 = extractvalue { ptr, i64 } %t104, 1
  %t107 = icmp eq i64 %t106, 0
  br i1 %t107, label %L49, label %L47
L49:
  %t108 = icmp eq ptr %t105, null
  br i1 %t108, label %L50, label %L48
L50:
  call void @cool_null_fault_at(ptr @.str.21)
  br label %L48
L47:
  %t109 = getelementptr i8, ptr %t105, i64 -8
  %t110 = load atomic i64, ptr %t109 seq_cst, align 8
  %t111 = icmp ne i64 %t110, %t106
  br i1 %t111, label %L51, label %L48
L51:
  call void @cool_gen_fault_at(ptr @.str.21)
  br label %L48
L48:
  %t112 = getelementptr %Vector$Pair, ptr %t105, i32 0, i32 1
  %t113 = load i64, ptr %t112
  %t114 = load { ptr, i64 }, ptr %t0
  %t115 = extractvalue { ptr, i64 } %t114, 0
  %t116 = extractvalue { ptr, i64 } %t114, 1
  %t117 = icmp eq i64 %t116, 0
  br i1 %t117, label %L54, label %L52
L54:
  %t118 = icmp eq ptr %t115, null
  br i1 %t118, label %L55, label %L53
L55:
  call void @cool_null_fault_at(ptr @.str.21)
  br label %L53
L52:
  %t119 = getelementptr i8, ptr %t115, i64 -8
  %t120 = load atomic i64, ptr %t119 seq_cst, align 8
  %t121 = icmp ne i64 %t120, %t116
  br i1 %t121, label %L56, label %L53
L56:
  call void @cool_gen_fault_at(ptr @.str.21)
  br label %L53
L53:
  %t122 = getelementptr %Vector$Pair, ptr %t115, i32 0, i32 0
  %t123 = load ptr, ptr %t122
  %t124 = getelementptr %Pair, ptr %t123, i64 %t113
  %t125 = load %Pair, ptr %t1
  store %Pair %t125, ptr %t124
  %t126 = load { ptr, i64 }, ptr %t0
  %t127 = extractvalue { ptr, i64 } %t126, 0
  %t128 = extractvalue { ptr, i64 } %t126, 1
  %t129 = icmp eq i64 %t128, 0
  br i1 %t129, label %L59, label %L57
L59:
  %t130 = icmp eq ptr %t127, null
  br i1 %t130, label %L60, label %L58
L60:
  call void @cool_null_fault_at(ptr @.str.22)
  br label %L58
L57:
  %t131 = getelementptr i8, ptr %t127, i64 -8
  %t132 = load atomic i64, ptr %t131 seq_cst, align 8
  %t133 = icmp ne i64 %t132, %t128
  br i1 %t133, label %L61, label %L58
L61:
  call void @cool_gen_fault_at(ptr @.str.22)
  br label %L58
L58:
  %t134 = getelementptr %Vector$Pair, ptr %t127, i32 0, i32 1
  %t135 = load { ptr, i64 }, ptr %t0
  %t136 = extractvalue { ptr, i64 } %t135, 0
  %t137 = extractvalue { ptr, i64 } %t135, 1
  %t138 = icmp eq i64 %t137, 0
  br i1 %t138, label %L64, label %L62
L64:
  %t139 = icmp eq ptr %t136, null
  br i1 %t139, label %L65, label %L63
L65:
  call void @cool_null_fault_at(ptr @.str.22)
  br label %L63
L62:
  %t140 = getelementptr i8, ptr %t136, i64 -8
  %t141 = load atomic i64, ptr %t140 seq_cst, align 8
  %t142 = icmp ne i64 %t141, %t137
  br i1 %t142, label %L66, label %L63
L66:
  call void @cool_gen_fault_at(ptr @.str.22)
  br label %L63
L63:
  %t143 = getelementptr %Vector$Pair, ptr %t136, i32 0, i32 1
  %t144 = load i64, ptr %t143
  %t145 = add i64 %t144, 1
  store i64 %t145, ptr %t134
  ret void
}

define %Vector$Pair @vec_new$Pair() {
entry:
  %t0 = call ptr @cool_gen_alloc(i64 0)
  %t1 = insertvalue %Vector$Pair undef, ptr %t0, 0
  %t2 = insertvalue %Vector$Pair %t1, i64 0, 1
  %t3 = insertvalue %Vector$Pair %t2, i64 0, 2
  ret %Vector$Pair %t3
}

