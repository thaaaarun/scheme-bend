(define (parallel-sum lo hi)
  (if (= lo hi)
      lo
      (let ((mid (/ (+ lo hi) 2)))
        (+ (parallel-sum lo mid)
           (parallel-sum (+ mid 1) hi)))))

(parallel-sum 1 1000000)
