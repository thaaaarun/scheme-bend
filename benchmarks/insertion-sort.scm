(define (build-down i acc)
  (if (= i 0) acc (build-down (- i 1) (cons i acc))))

(define (insert x xs)
  (if (null? xs)
      (cons x ())
      (if (< x (car xs))
          (cons x xs)
          (cons (car xs) (insert x (cdr xs))))))

(define (isort xs)
  (if (null? xs)
      ()
      (insert (car xs) (isort (cdr xs)))))

(define (weighted xs i acc)
  (if (null? xs)
      acc
      (weighted (cdr xs) (+ i 1) (+ acc (* i (car xs))))))

(define (once) (weighted (isort (build-down 200 ())) 0 0))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 260 0)
