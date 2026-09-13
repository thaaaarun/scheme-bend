(define (mod a m) (- a (* (/ a m) m)))

(define (powmod b e m acc)
  (if (= e 0)
      acc
      (if (= (* (/ e 2) 2) e)
          (powmod (mod (* b b) m) (/ e 2) m acc)
          (powmod b (- e 1) m (mod (* acc b) m)))))

(define (once) (powmod 3 500000 997 1))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 138376 0)
