; Fixed-point escape-time Mandelbrot, checksummed over a 256x256 grid.
; Same algorithm and numeric semantics as benchmarks/mandelbrot.lisp.
;
; Coordinates carry 8 fractional bits (S = 256):
;   cr = -2.0 + px * 3.0 / 256      (px in [0,256), so x in [-2.0, 1.0))
;   ci = -1.25 + py * 2.5 / 256     (py in [0,256), so y in [-1.25, 1.25))
;
; scheme-bend emits every integer as Bend I24, so arithmetic, comparisons, and
; division are consistently signed. Every intermediate stays within +/-2^23.
; Bend's signed division truncates toward zero, matching CL's `truncate` below.
;
; `step-re` and `step-im` deliberately remain tiny named helpers in the source.
; The optimizer specializes their direct call sites inside `escape` so the hot
; loop has no helper-call rewrites.
(define (step-re zr zi cr)
  (+ (/ (- (* zr zr) (* zi zi)) 256) cr))

(define (step-im zr zi ci)
  (+ (/ (* 2 (* zr zi)) 256) ci))

(define (escape zr zi cr ci n)
  (if (= n 255)
      255
      (if (> (+ (* zr zr) (* zi zi)) 262144)
          n
          (escape (step-re zr zi cr) (step-im zr zi ci) cr ci (+ n 1)))))

(define (pixel p)
  (let ((px (- p (* (/ p 256) 256)))
        (py (/ p 256)))
    (escape 0 0 (+ -512 (/ (* px 768) 256)) (+ -320 (/ (* py 640) 256)) 0)))

(define (count-range lo hi)
  (if (= lo hi)
      (pixel lo)
      (let ((mid (/ (+ lo hi) 2)))
        (+ (count-range lo mid) (count-range (+ mid 1) hi)))))

(count-range 0 65535)
