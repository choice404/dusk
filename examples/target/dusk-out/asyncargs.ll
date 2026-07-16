; ModuleID = 'dusk'
target triple = "x86_64-pc-linux-gnu"

%Future$int64 = type { ptr, i64 }

@async.compute.framesize = constant i64 48
@.str.0 = private unnamed_addr constant [7 x i8] c"value=\00"
@.str.1 = private unnamed_addr constant [31 x i8] c"the event loop could not start\00"

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
declare ptr @cool_future_new(i64)
declare i64 @cool_future_gen(ptr)
declare i64 @cool_future_complete(ptr, i64, ptr, ptr)
declare i64 @cool_future_try(ptr, i64, ptr, ptr)
declare void @cool_future_wait(ptr, i64, ptr, ptr)
declare i64 @cool_future_await_ms(ptr, i64, i64, ptr, ptr)
declare void @cool_future_release(ptr, i64)
declare i64 @cool_loop_init()
declare void @cool_loop_free()

define void @async.compute.poll(ptr %frame) {
entry:
  %t0 = getelementptr i8, ptr %frame, i64 8
  %t1 = getelementptr i8, ptr %frame, i64 16
  %t2 = getelementptr i8, ptr %frame, i64 24
  %t3 = getelementptr i8, ptr %frame, i64 32
  %t4 = getelementptr i8, ptr %frame, i64 40
  %t9 = load i64, ptr %frame
  switch i64 %t9, label %badstate [ i64 0, label %start ]
badstate:
  call void @cool_task_state_fault()
  unreachable
start:
  %t5 = load ptr, ptr %t4
  call void @cool_print_cstr(ptr %t5)
  %t6 = load i64, ptr %t3
  call void @cool_println_i64(i64 %t6)
  %t7 = load i64, ptr %t3
  %t8 = mul i64 %t7, 2
  store i64 %t8, ptr %t2
  call void @cool_task_return(ptr %frame, ptr %t2, i64 8)
  ret void
}

define i32 @main() {
entry:
  %t0 = alloca i8
  %t2 = alloca ptr
  %t15 = alloca i64
  %t17 = alloca i64
  call void @cool_gc_anchor(ptr %t0)
  %t1 = call ptr @loop_init()
  store ptr %t1, ptr %t2
  %t3 = load ptr, ptr %t2
  %t4 = load i64, ptr @async.compute.framesize
  %t5 = call ptr @cool_task_new(ptr @async.compute.poll, i64 %t4, i64 8)
  %t6 = call ptr @cool_task_frame(ptr %t5)
  %t7 = getelementptr i8, ptr %t6, i64 32
  store i64 21, ptr %t7
  %t8 = getelementptr i8, ptr %t6, i64 40
  store ptr @.str.0, ptr %t8
  call void @cool_task_start(ptr %t5)
  %t9 = getelementptr i8, ptr %t5, i64 -8
  %t10 = load atomic i64, ptr %t9 seq_cst, align 8
  %t11 = insertvalue %Future$int64 undef, ptr %t5, 0
  %t12 = insertvalue %Future$int64 %t11, i64 %t10, 1
  %t13 = extractvalue %Future$int64 %t12, 0
  %t14 = extractvalue %Future$int64 %t12, 1
  call void @cool_loop_run(ptr %t13, i64 %t14, ptr %t15, i64 8)
  %t16 = load i64, ptr %t15
  store i64 %t16, ptr %t17
  call void @loop_free()
  %t18 = load i64, ptr %t17
  call void @cool_println_i64(i64 %t18)
  %t19 = trunc i64 0 to i32
  ret i32 %t19
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

