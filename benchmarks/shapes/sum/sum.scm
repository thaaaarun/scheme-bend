; Sum of [1, N], written as a linear accumulator chain.
;
; No explicit modulus is needed: the target's arithmetic is signed 24-bit and
; wraps, and that wrap *is* the modulus. The shape pass relies on the same fact
; -- wrapping arithmetic is a ring, so `+` re-associates freely.
(define (run i n acc)
  (if (> i n)
      acc
      (run (+ i 1) n (+ acc i))))
(run 1 2000000 0)
