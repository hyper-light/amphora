-- Cancellation: 1e16 + 1 - 1e16 (sqlite.org/floatingpoint.html), spread over every order.
SELECT sum(CASE l_linenumber WHEN 1 THEN 1e16 WHEN 2 THEN -1e16 ELSE 1.0 END), avg(CASE l_linenumber WHEN 1 THEN 1e16 WHEN 2 THEN -1e16 ELSE 1.0 END), total(CASE l_linenumber WHEN 1 THEN 1e16 WHEN 2 THEN -1e16 ELSE 1.0 END) FROM lineitem;
