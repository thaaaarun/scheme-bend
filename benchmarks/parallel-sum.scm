; Balanced divide-and-conquer: after `mid` is computed, both recursive calls
; have no data dependency and are eligible to run in parallel under HVM.
; Bend integers are 24-bit, so the final sum intentionally wraps modulo 2^24.
(define (parallel-sum lo hi)
  (if (= lo hi)
      lo
      (let ((mid (/ (+ lo hi) 2)))
        (+ (parallel-sum lo mid)
           (parallel-sum (+ mid 1) hi)))))
(parallel-sum 1 1000000)
