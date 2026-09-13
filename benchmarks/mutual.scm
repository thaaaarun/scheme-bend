(define (my-even n) (if (= n 0) 1 (my-odd (- n 1))))

(define (my-odd n) (if (= n 0) 0 (my-even (- n 1))))

(define (once) (my-even 20000))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 536 0)
