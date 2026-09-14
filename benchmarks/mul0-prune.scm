; Regression workload for the sparse-kernel annihilation primitive. The first
; operand reaches zero; the second operand is deliberately expensive. A
; correct lazy `mul0` should not demand the second recurrence.

(define (zero-after n)
  (if (= n 0)
      0
      (zero-after (- n 1))))

(define (work n acc)
  (if (= n 0)
      acc
      (work (- n 1) (+ acc n))))

(mul0 (zero-after 32) (work 100000 0))
