(define (ss i n acc)
  (if (> i n)
      acc
      (ss (+ i 1) n (+ acc (* i i)))))

(define (once) (ss 1 200 0))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 31127 0)
