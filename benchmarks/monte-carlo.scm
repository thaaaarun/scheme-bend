; Monte Carlo: price a European call by simulating 6144 independent arithmetic
; random walks of 1024 steps and summing the payoffs.
;
; Parallel shape: paths are completely independent -- no path depends on any
; other -- so this is the cleanest probe of how far the backend scales. The
; work sits in 6144 heavy leaves, each a 1024-step sequential walk, and the
; only reduction is the final sum over paths.
;
; Bounds, all deliberately inside signed 24-bit so nothing wraps:
;   * the generator is s -> (s * 1277 + 12345) mod 6553, and
;     1277 * 6552 + 12345 = 8,379,249 < 8,388,607, so the state cannot wrap;
;   * seed p stays below 6553 for p <= 6144, so every path gets its own stream;
;   * a walk of K up/down steps is in [-K, K], so the payoff is in [0, 1024];
;   * 6144 * 1024 = 6,291,456 < 8,388,607, so the sum over paths cannot wrap.
;
; Because every intermediate stays in range, the Bend and SBCL results must
; agree exactly rather than only modulo 2^24 -- unlike the frozen benchmarks,
; which suite.sh has to normalise.
(define (mod- a m) (- a (* (/ a m) m)))

(define (next s) (mod- (+ (* s 1277) 12345) 6553))

(define (dir s) (if (= (mod- s 2) 0) 1 (- 0 1)))

; Sum of `k` up/down steps starting from seed `s`.
(define (walk k s acc)
  (if (= k 0)
      acc
      (walk (- k 1) (next s) (+ acc (dir s)))))

; Terminal value less the strike, floored at zero. Start and strike are both
; 1024, so this is max(total move, 0) and lies in [0, 1024].
(define (payoff p)
  (let ((move (walk 1024 p 0)))
    (if (> move 0) move 0)))

; Sum of payoffs over every path. A bare `+`, so the shape pass may re-associate
; it; that is sound here precisely because the bound above holds.
(define (run i n acc)
  (if (> i n)
      acc
      (run (+ i 1) n (+ acc (payoff i)))))

(run 1 6144 0)
