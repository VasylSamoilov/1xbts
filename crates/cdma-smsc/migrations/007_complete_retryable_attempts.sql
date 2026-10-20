UPDATE sms_delivery_attempts AS attempt
SET state = 'failed',
    failure_reason = COALESCE(attempt.failure_reason, 'retryable submission had an unfinished delivery attempt'),
    completed_at = NOW(),
    updated_at = NOW()
FROM sms_submissions AS submission
WHERE attempt.sms_id = submission.sms_id
  AND submission.state = 'accepted'
  AND attempt.state = 'paging'
  AND attempt.attempt_number = (
      SELECT MAX(previous.attempt_number)
      FROM sms_delivery_attempts AS previous
      WHERE previous.sms_id = attempt.sms_id
  );
