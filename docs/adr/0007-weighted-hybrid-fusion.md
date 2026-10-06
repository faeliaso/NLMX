# ADR 0007 — Weighted hybrid fusion instead of RRF

- Status: accepted
- Date: 2026-10-02
- Supersedes: the RRF fusion of ADR 0004.

## Context
The plan was to fuse BM25 and KNN via Reciprocal Rank Fusion. RRF uses only each result's position, so the final score does not say how close the passage is to the question. The RAG needs an absolute score for the *relevance gate* ("not found" without calling the model).

## Decision
`domain::retrieval::fuse` combines the two mechanisms by a weighted average of normalized scores:
- semantic = cosine similarity clamped to [0, 1] (absolute);
- lexical = min-max of −bm25 over the candidates × **squared coverage** (fraction of the query terms present in the chunk), so that 1 term out of 3 does not get 100%;
- final = `(w_s·s + w_l·l) / (w_s + w_l)`, ties broken by semantic score and then id. Weight 0 disables a mechanism.

## Consequences
- The semantic score keeps an absolute meaning; the RAG's `min_relevance` is a stable threshold.
- Weights need calibration; the comparison between the mechanisms was an integration test that has been removed; recalibrate by measuring again.
