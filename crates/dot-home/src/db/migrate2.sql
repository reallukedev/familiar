-- Approvals carry their full arguments so the owner sees exactly what they approve.
ALTER TABLE approvals ADD COLUMN arguments TEXT;
