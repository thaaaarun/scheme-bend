(define (ack m n)
  (if (= m 0)
      (+ n 1)
      (if (= n 0)
          (ack (- m 1) 1)
          (ack (- m 1) (ack m (- n 1))))))

(define (once) (ack 3 5))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 196 0)
