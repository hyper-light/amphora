# diff Wrong: trigger_strict_before_update

plain

## Statements

```sql
CREATE TABLE n(id INTEGER PRIMARY KEY, v INTEGER) STRICT;
CREATE TABLE audit(v);
CREATE TRIGGER g BEFORE UPDATE ON n BEGIN INSERT INTO audit VALUES (new.v); END;
INSERT INTO n VALUES (1, 2);
UPDATE n SET v = '7' WHERE id = 1;
```

## Queries

```sql
SELECT v, typeof(v) FROM audit;
SELECT * FROM n;
```

## What differed

```
```
