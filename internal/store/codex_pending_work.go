package store

import (
	"context"

	"github.com/wbbradley/hq/internal/domain"
	"github.com/wbbradley/hq/internal/model"
)

// ListCodexPendingWork returns one durable launch target per inbox with work
// that its currently selected Codex thread can consume.
func (s *SQLite) ListCodexPendingWork(ctx context.Context) ([]domain.CodexPendingWork, error) {
	rows, err := s.db.QueryContext(ctx, `
SELECT 'direct-agent',n.name,n.mailbox_id,n.current_session_id,
       COALESCE(a.directory,''),COALESCE(a.git_common_dir,''),COALESCE(a.remote_identity,''),COALESCE(a.worktree,''),COALESCE(a.branch,''),
       '','',''
FROM named_agents n
JOIN agent_sessions a ON a.agent_name=n.name AND a.harness=n.current_harness AND a.external_session_id=n.current_session_id
WHERE n.retired=0 AND n.current_harness='codex' AND n.current_session_id<>''
  AND NOT EXISTS (SELECT 1 FROM project_assignment_epochs e WHERE e.agent_name=n.name AND e.ended_event_id IS NULL)
  AND EXISTS (
      SELECT 1 FROM messages m JOIN delivery_facts d ON d.message_id=m.id
      WHERE m.recipient_mailbox_id=n.mailbox_id AND d.completed_at IS NULL
        AND NOT EXISTS (SELECT 1 FROM projects p WHERE p.mailbox_id=m.recipient_mailbox_id)
        AND (m.reply_to IS NULL OR instr(char(10)||m.details||char(10),char(10)||'Codex thread: '||n.current_session_id||char(10))>0)
  )
UNION ALL
SELECT 'project-assignment',e.agent_name,p.mailbox_id,t.external_thread_id,
       t.launch_directory,'','','','',p.id,e.id,t.id
FROM projects p
JOIN project_assignment_epochs e ON e.project_id=p.id AND e.ended_event_id IS NULL
JOIN project_threads t ON t.id=e.selected_thread_id AND t.project_id=p.id AND t.agent_name=e.agent_name
JOIN named_agents n ON n.name=e.agent_name
WHERE p.home_installation_id=? AND p.lifecycle='open' AND p.archived=0 AND e.state='runnable' AND t.harness='codex' AND n.retired=0
  AND EXISTS (
      SELECT 1 FROM project_message_acceptances x JOIN delivery_facts d ON d.message_id=x.message_id
      WHERE x.project_id=p.id AND d.completed_at IS NULL
  )
ORDER BY 1,2,10`, s.signer.InstallationID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	var result []domain.CodexPendingWork
	for rows.Next() {
		var item domain.CodexPendingWork
		var directory, gitCommon, remote, worktree, branch string
		if err := rows.Scan(&item.Kind, &item.AgentName, &item.MailboxID, &item.SessionID, &directory, &gitCommon, &remote, &worktree, &branch, &item.ProjectID, &item.AssignmentID, &item.ProjectThreadID); err != nil {
			return nil, err
		}
		item.Repository = model.RepositoryContext{Directory: directory, GitCommonDir: gitCommon, RemoteIdentity: remote, Worktree: worktree, Branch: branch}
		result = append(result, item)
	}
	if err := rows.Err(); err != nil {
		return nil, err
	}
	return result, nil
}

var _ domain.CodexPendingWorkOperations = (*SQLite)(nil)
