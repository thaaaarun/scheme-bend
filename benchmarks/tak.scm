(define (tak x y z)
  (if (< y x)
      (tak (tak (- x 1) y z)
           (tak (- y 1) z x)
           (tak (- z 1) x y))
      z))

(define (once) (tak 18 12 6))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 119 0)
