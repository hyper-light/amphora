-- Signed zeros: every input -0.0, and +0.0 on first lines only.
SELECT sum(-0.0), avg(-0.0), total(-0.0), sum(CASE l_linenumber WHEN 1 THEN 0.0 ELSE -0.0 END) FROM lineitem;
