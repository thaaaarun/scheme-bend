(define (build i n acc)
  (if (> i n) acc (build (+ i 1) n (cons i acc))))

(define (rev xs acc)
  (if (null? xs) acc (rev (cdr xs) (cons (car xs) acc))))

(define (sum xs acc)
  (if (null? xs) acc (sum (cdr xs) (+ acc (car xs)))))

(define (once) (sum (rev (build 1 2000 ()) ()) 0))

(define (mod- a m) (- a (* (/ a m) m)))

(define (repeat k acc)
  (if (= k 0)
      acc
      (repeat (- k 1) (mod- (+ acc (mod- (once) 4000000)) 4000000))))

(repeat 1153 0)
