-- KI-324: a group delete left the group's role grants behind. An
-- `access_grants` row names its principal by id with no foreign key, and until
-- `GroupRepo::delete_local` removed them in its own transaction the delete
-- removed only the group and (by cascade) its memberships.
--
-- Such a row grants nobody anything: the members went with the group, and a
-- group id is a random uuid no later group is given, so deleting it changes
-- no one's access. Only `principal_type = 'group'`: user and service rows are
-- out of scope here.
DELETE FROM access_grants ag
 WHERE ag.principal_type = 'group'
   AND NOT EXISTS (SELECT 1 FROM groups g WHERE g.id = ag.principal_id);
