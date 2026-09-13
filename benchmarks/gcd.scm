(define (gcd a b)
  (if (= b 0)
      a
      (gcd b (- a (* (/ a b) b)))))

(define (sweep i n acc)
  (if (> i n)
      acc
      (sweep (+ i 1) n (+ acc (gcd (+ 1000 i) (+ 2000 (* 2 i)))))))

(define (once) (sweep 1 1000 0))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 1546 0)
