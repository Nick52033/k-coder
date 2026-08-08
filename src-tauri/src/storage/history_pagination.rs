use super::*;

pub(crate) fn paginate_indexed_turns(
    projection: &ProjectionDb,
    thread_id: &str,
    cursor: Option<&str>,
    limit: Option<u32>,
    sort_direction: HistorySortDirection,
    items_view: TurnItemsView,
) -> Result<ThreadTurnsPage, StorageError> {
    let limit = history_page_limit(limit)?;
    let cursor = decode_history_cursor(cursor, thread_id, HistoryCursorResource::Turns, None)?;
    if let Some(cursor) = &cursor
        && !projection
            .history_turn_exists(
                thread_id,
                history_index_order(cursor.order),
                &cursor.anchor_id,
            )
            .map_err(projection_error)?
    {
        return Err(invalid_history_cursor());
    }
    let mut candidates = projection
        .history_turn_page(
            thread_id,
            cursor
                .as_ref()
                .map(|cursor| (history_index_order(cursor.order), cursor.inclusive)),
            limit + 1,
            sort_direction,
        )
        .map_err(projection_error)?;
    let has_more = candidates.len() > limit;
    candidates.truncate(limit);
    let data = candidates
        .iter()
        .map(|turn| turn_with_items_view(&turn.turn, items_view))
        .collect::<Vec<_>>();
    let next_cursor = if has_more {
        candidates.last().map(|turn| {
            encode_history_cursor(HistoryCursor {
                version: 1,
                thread_id: thread_id.to_string(),
                resource: HistoryCursorResource::Turns,
                anchor_id: turn.turn.id.clone(),
                order: projected_history_order(turn.order),
                inclusive: false,
                filter_turn_id: None,
            })
        })
    } else {
        None
    };
    let backwards_cursor = candidates.first().map(|turn| {
        encode_history_cursor(HistoryCursor {
            version: 1,
            thread_id: thread_id.to_string(),
            resource: HistoryCursorResource::Turns,
            anchor_id: turn.turn.id.clone(),
            order: projected_history_order(turn.order),
            inclusive: true,
            filter_turn_id: None,
        })
    });
    Ok(ThreadTurnsPage {
        data,
        next_cursor,
        backwards_cursor,
    })
}

pub(crate) fn paginate_indexed_items(
    projection: &ProjectionDb,
    thread_id: &str,
    turn_id: Option<&str>,
    cursor: Option<&str>,
    limit: Option<u32>,
    sort_direction: HistorySortDirection,
) -> Result<ThreadItemsPage, StorageError> {
    if let Some(turn_id) = turn_id
        && !projection
            .history_has_turn(thread_id, turn_id)
            .map_err(projection_error)?
    {
        return Err(StorageError::NotFound(format!(
            "turn {turn_id} in thread {thread_id}"
        )));
    }
    let limit = history_page_limit(limit)?;
    let cursor = decode_history_cursor(cursor, thread_id, HistoryCursorResource::Items, turn_id)?;
    if let Some(cursor) = &cursor
        && !projection
            .history_item_exists(
                thread_id,
                history_index_order(cursor.order),
                &cursor.anchor_id,
                turn_id,
            )
            .map_err(projection_error)?
    {
        return Err(invalid_history_cursor());
    }
    let mut candidates = projection
        .history_item_page(
            thread_id,
            turn_id,
            cursor
                .as_ref()
                .map(|cursor| (history_index_order(cursor.order), cursor.inclusive)),
            limit + 1,
            sort_direction,
        )
        .map_err(projection_error)?;
    let has_more = candidates.len() > limit;
    candidates.truncate(limit);
    let data = candidates
        .iter()
        .map(|item| ThreadItemEntry {
            turn_id: item.item.turn_id.clone(),
            item: item.item.clone(),
        })
        .collect::<Vec<_>>();
    let next_cursor = if has_more {
        candidates.last().map(|item| {
            encode_history_cursor(HistoryCursor {
                version: 1,
                thread_id: thread_id.to_string(),
                resource: HistoryCursorResource::Items,
                anchor_id: item.item.id.clone(),
                order: projected_history_order(item.order),
                inclusive: false,
                filter_turn_id: turn_id.map(str::to_string),
            })
        })
    } else {
        None
    };
    let backwards_cursor = candidates.first().map(|item| {
        encode_history_cursor(HistoryCursor {
            version: 1,
            thread_id: thread_id.to_string(),
            resource: HistoryCursorResource::Items,
            anchor_id: item.item.id.clone(),
            order: projected_history_order(item.order),
            inclusive: true,
            filter_turn_id: turn_id.map(str::to_string),
        })
    });
    Ok(ThreadItemsPage {
        data,
        next_cursor,
        backwards_cursor,
    })
}

fn turn_with_items_view(turn: &ThreadTurn, items_view: TurnItemsView) -> ThreadTurn {
    let mut turn = turn.clone();
    turn.items_view = items_view;
    match items_view {
        TurnItemsView::NotLoaded => turn.items.clear(),
        TurnItemsView::Summary => turn.items.retain(|item| {
            matches!(&item.payload, ThreadItemPayload::UserMessage { .. })
                || matches!(
                    &item.payload,
                    ThreadItemPayload::AgentMessage {
                        phase: AgentMessagePhase::FinalAnswer,
                        ..
                    }
                )
        }),
        TurnItemsView::Full => {}
    }
    turn
}

fn history_page_limit(limit: Option<u32>) -> Result<usize, StorageError> {
    let limit = limit.unwrap_or(DEFAULT_THREAD_HISTORY_PAGE_SIZE);
    if !(1..=MAX_THREAD_HISTORY_PAGE_SIZE).contains(&limit) {
        return Err(StorageError::InvalidData(format!(
            "history page limit must be between 1 and {MAX_THREAD_HISTORY_PAGE_SIZE}"
        )));
    }
    Ok(limit as usize)
}

fn encode_history_cursor(cursor: HistoryCursor) -> String {
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).expect("history cursor should serialize"))
}

fn decode_history_cursor(
    cursor: Option<&str>,
    thread_id: &str,
    resource: HistoryCursorResource,
    filter_turn_id: Option<&str>,
) -> Result<Option<HistoryCursor>, StorageError> {
    let Some(cursor) = cursor else {
        return Ok(None);
    };
    let decoded = URL_SAFE_NO_PAD
        .decode(cursor)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<HistoryCursor>(&bytes).ok())
        .filter(|cursor| {
            cursor.version == 1
                && cursor.thread_id == thread_id
                && cursor.resource == resource
                && cursor.filter_turn_id.as_deref() == filter_turn_id
        })
        .ok_or_else(invalid_history_cursor)?;
    Ok(Some(decoded))
}

fn invalid_history_cursor() -> StorageError {
    StorageError::InvalidData("invalid or stale thread history cursor".to_string())
}
