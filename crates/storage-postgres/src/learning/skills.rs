use super::{
    AssessmentInput, PgConnection, PostgresStore, Row, SelfAssessment, SkillInput, SkillNode,
    StorageError, StorageResult, UserId, Uuid, bump, conflict, graph, id, integer, invalid, locked,
    map_error, normalize_skill_graph, now, number, rating, revision,
};

pub(super) async fn save(
    store: &PostgresStore,
    owner: Uuid,
    key: Uuid,
    expected: u64,
    input: SkillInput,
) -> StorageResult<SkillNode> {
    integer(expected)?;
    let mut tx = locked(store, owner).await?;
    let mut nodes = graph(&mut tx, owner).await?;
    let previous = nodes
        .iter()
        .find(|n| n.skill_id == key.to_string())
        .cloned();
    let next = expected
        .checked_add(1)
        .filter(|v| i64::try_from(*v).is_ok())
        .ok_or_else(conflict)?;
    let proposed = SkillNode {
        user_id: owner.to_string(),
        skill_id: key.to_string(),
        revision: next,
        name: input.name,
        enabled: input.enabled,
        deleted: false,
        prerequisite_ids: input.prerequisite_ids,
    };
    nodes.retain(|n| n.skill_id != proposed.skill_id);
    nodes.push(proposed);
    if nodes.len() > 100 {
        return Err(conflict());
    }
    let normalized =
        normalize_skill_graph(&UserId::new(owner.to_string()), &nodes).map_err(|_| invalid())?;
    let saved = normalized
        .into_iter()
        .find(|n| n.skill_id == key.to_string())
        .ok_or_else(invalid)?;
    if let Some(old) = previous {
        if old.deleted || (expected != old.revision && !(expected == 0 && old.revision == 1)) {
            return Err(conflict());
        }
        let same = old.name == saved.name
            && old.enabled == saved.enabled
            && old.prerequisite_ids == saved.prerequisite_ids;
        if same {
            return Ok(old);
        }
        if expected == 0 {
            return Err(conflict());
        }
    } else if expected != 0 {
        return Err(StorageError::NotFound);
    }
    sqlx::query("INSERT INTO learning_skills(user_id,id,revision,name,enabled) VALUES($1,$2,$3,$4,$5) ON CONFLICT(user_id,id) DO UPDATE SET revision=EXCLUDED.revision,name=EXCLUDED.name,enabled=EXCLUDED.enabled")
        .bind(owner).bind(key).bind(integer(saved.revision)?).bind(&saved.name).bind(saved.enabled).execute(&mut *tx).await.map_err(map_error)?;
    sqlx::query("DELETE FROM learning_edges WHERE user_id=$1 AND skill_id=$2")
        .bind(owner)
        .bind(key)
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    for parent in &saved.prerequisite_ids {
        sqlx::query(
            "INSERT INTO learning_edges(user_id,skill_id,prerequisite_id) VALUES($1,$2,$3)",
        )
        .bind(owner)
        .bind(key)
        .bind(id(parent)?)
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    }
    bump(&mut tx, owner).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(saved)
}
pub(super) async fn delete(
    store: &PostgresStore,
    owner: Uuid,
    key: Uuid,
    expected: u64,
) -> StorageResult<()> {
    integer(expected)?;
    let mut tx = locked(store, owner).await?;
    let row =
        sqlx::query("SELECT revision,deleted FROM learning_skills WHERE user_id=$1 AND id=$2")
            .bind(owner)
            .bind(key)
            .fetch_one(&mut *tx)
            .await
            .map_err(map_error)?;
    let current = number(row.get("revision"))?;
    if row.get::<bool, _>("deleted") {
        return if expected == current || expected.checked_add(1) == Some(current) {
            Ok(())
        } else {
            Err(conflict())
        };
    }
    if expected != current || current >= i64::MAX as u64 {
        return Err(conflict());
    }
    sqlx::query("UPDATE learning_skills SET name='已删除技能',enabled=false,deleted=true,revision=revision+1 WHERE user_id=$1 AND id=$2")
        .bind(owner).bind(key).execute(&mut *tx).await.map_err(map_error)?;
    sqlx::query("DELETE FROM learning_edges WHERE user_id=$1 AND skill_id=$2")
        .bind(owner)
        .bind(key)
        .execute(&mut *tx)
        .await
        .map_err(map_error)?;
    bump(&mut tx, owner).await?;
    tx.commit().await.map_err(map_error)
}
async fn assessment_count(tx: &mut PgConnection, owner: Uuid) -> StorageResult<()> {
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM learning_assessments WHERE user_id=$1")
            .bind(owner)
            .fetch_one(tx)
            .await
            .map_err(map_error)?;
    if count >= 1000 {
        return Err(conflict());
    }
    Ok(())
}
pub(super) async fn assess(
    store: &PostgresStore,
    owner: Uuid,
    key: Uuid,
    input: AssessmentInput,
) -> StorageResult<SelfAssessment> {
    let mut tx = locked(store, owner).await?;
    let saved = assess_locked(&mut tx, owner, key, input).await?;
    tx.commit().await.map_err(map_error)?;
    Ok(saved)
}
pub(super) async fn assess_locked(
    tx: &mut PgConnection,
    owner: Uuid,
    key: Uuid,
    input: AssessmentInput,
) -> StorageResult<SelfAssessment> {
    let skill = id(&input.skill_id)?;
    integer(input.expected_revision)?;
    integer(input.skill_revision)?;
    if input.score > 100 || input.skill_revision == 0 {
        return Err(invalid());
    }
    if let Some(row) = sqlx::query("SELECT * FROM learning_assessments WHERE user_id=$1 AND id=$2")
        .bind(owner)
        .bind(key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(map_error)?
    {
        let saved = rating(&row)?;
        if saved.skill_id != skill.to_string()
            || saved.skill_revision != input.skill_revision
            || saved.score != input.score
            || number(row.get("expected_revision"))? != input.expected_revision
        {
            return Err(conflict());
        }
        return Ok(saved);
    }
    if revision(tx, owner).await? != input.expected_revision {
        return Err(conflict());
    }
    let row = sqlx::query(
        "SELECT revision,enabled,deleted FROM learning_skills WHERE user_id=$1 AND id=$2",
    )
    .bind(owner)
    .bind(skill)
    .fetch_one(&mut *tx)
    .await
    .map_err(map_error)?;
    if number(row.get("revision"))? != input.skill_revision
        || !row.get::<bool, _>("enabled")
        || row.get::<bool, _>("deleted")
    {
        return Err(conflict());
    }
    assessment_count(tx, owner).await?;
    let time = now(tx).await?;
    let row=sqlx::query("INSERT INTO learning_assessments(user_id,id,skill_id,skill_revision,expected_revision,score,assessed_ms) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING *")
        .bind(owner).bind(key).bind(skill).bind(integer(input.skill_revision)?).bind(integer(input.expected_revision)?).bind(i16::from(input.score)).bind(time).fetch_one(&mut *tx).await.map_err(map_error)?;
    let saved = rating(&row)?;
    bump(tx, owner).await?;
    Ok(saved)
}
