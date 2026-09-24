VERDICT: better

The extraction is genuine, not metric gaming: the three functions were truly identical except for which key they grouped by, `keyOf` names that variation explicitly, and each call site still reads as a one-line description of intent (by category / by country / by tier). The `??`-based accumulation is a minor, harmless tightening of the same logic. No indirection was added that doesn't pay for itself — there's exactly one shared loop and three thin, self-explanatory callers, so this is legitimate DRY rather than a contrived abstraction to lower a duplication score.
