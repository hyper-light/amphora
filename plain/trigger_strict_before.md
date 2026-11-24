# diff Wrong: trigger_strict_before

plain

## Statements

```sql
CREATE TABLE price(id INTEGER PRIMARY KEY, amount REAL) STRICT;
CREATE TABLE audit(amount);
CREATE TRIGGER g BEFORE INSERT ON price BEGIN INSERT INTO audit VALUES (new.amount); END;
INSERT INTO price VALUES (1, 100);
```

## Queries

```sql
SELECT amount, typeof(amount) FROM audit;
```

## What differed

```
```
