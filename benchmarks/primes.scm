(define (divides d n) (= (* (/ n d) d) n))

(define (prime? n d)
  (if (> (* d d) n)
      1
      (if (divides d n)
          0
          (prime? n (+ d 1)))))

(define (count-primes i n acc)
  (if (> i n)
      acc
      (count-primes (+ i 1) n (+ acc (prime? i 2)))))

(define (once) (count-primes 2 3000 0))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 149 0)
