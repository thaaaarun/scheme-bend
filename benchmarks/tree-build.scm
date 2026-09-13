(define (build d)
  (if (= d 0)
      ()
      (cons d (cons (build (- d 1)) (build (- d 1))))))

(define (tsum tree)
  (if (null? tree)
      0
      (+ (car tree) (+ (tsum (car (cdr tree))) (tsum (cdr (cdr tree)))))))

(define (once) (tsum (build 12)))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 97 0)
