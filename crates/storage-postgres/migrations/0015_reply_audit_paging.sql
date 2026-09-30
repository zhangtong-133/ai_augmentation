-- 按用户、UTC 日及币种读取有界审计明细；金额凭据仍保留至用户删除。
CREATE INDEX reply_money_reservations_audit_page
    ON reply_money_reservations(user_id, day, currency, conversation_id, request_id);
