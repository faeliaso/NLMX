-- Runtime vector tables (chunk_vectors_<model id>) are not known to this script: the app must drop
-- them with Database::drop_vector_table before downgrading past this version.
DROP TABLE embedding_jobs;
DROP TABLE embedding_models;
