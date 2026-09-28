;; vnengine demo plugin: a session journal tracker.
;; Counts menu choices, logs lifecycle, and stores the last
;; `plugin journal ...` command payload — all through the sandboxed
;; KV, so its state follows saves and rollback.
(module
  (import "env" "vn_log" (func $log (param i32 i32)))
  (import "env" "vn_kv_set" (func $set (param i32 i32 i32 i32)))
  (import "env" "vn_kv_get" (func $get (param i32 i32 i32 i32) (result i32)))
  (memory (export "memory") 1)
  ;; Persistent counter (plugin RAM — not saved; KV is the saved state).
  (global $choices (mut i32) (i32.const 0))
  ;; Static strings.
  (data (i32.const 0) "choices")           ;; 7  key
  (data (i32.const 16) "last_cmd")         ;; 8  key
  (data (i32.const 32) "journal ready")    ;; 13 log
  (data (i32.const 48) "cmd: ")            ;; 5  log prefix
  (data (i32.const 64) " ")                ;; 1  scratch (counter digit)

  (func (export "vn_init")
    (call $log (i32.const 32) (i32.const 13)))

  ;; Each player choice bumps the counter and records it in KV.
  (func (export "vn_on_choice") (param i32)
    (global.set $choices (i32.add (global.get $choices) (i32.const 1)))
    (i32.store8 (i32.const 64)
      (i32.add (i32.const 48) (global.get $choices)))  ;; '0' + n (n<=9)
    (call $set (i32.const 0) (i32.const 7)
               (i32.const 64) (i32.const 1)))

  ;; Fixed buffer for incoming command payloads.
  (func (export "vn_alloc") (param i32) (result i32)
    (i32.const 512))

  ;; `plugin journal <args...>` — keep the payload under "last_cmd".
  (func (export "vn_on_command") (param $ptr i32) (param $len i32)
    (call $set (i32.const 16) (i32.const 8)
               (local.get $ptr) (local.get $len)))
)
