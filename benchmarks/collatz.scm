(define (steps n count)
  (if (= n 1)
      count
      (if (= (* (/ n 2) 2) n)
          (steps (/ n 2) (+ count 1))
          (steps (+ (* 3 n) 1) (+ count 1)))))

(define (total i limit acc)
  (if (> i limit)
      acc
      (total (+ i 1) limit (+ acc (steps i 0)))))

(define (once) (total 1 1000 0))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 97 0)
