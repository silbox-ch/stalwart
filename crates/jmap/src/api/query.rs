/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use jmap_proto::{
    method::query::{QueryRequest, QueryResponse},
    object::JmapObject,
    types::state::State,
};
use types::id::Id;

pub struct QueryResponseBuilder {
    requested_position: i32,
    position: i32,
    pub limit: usize,
    anchor: u32,
    anchor_offset: i32,
    has_anchor: bool,
    anchor_found: bool,

    pub response: QueryResponse,
}

impl QueryResponseBuilder {
    pub fn new<T: JmapObject + Sync + Send>(
        total_results: usize,
        max_results: usize,
        query_state: State,
        request: &QueryRequest<T>,
    ) -> Self {
        let (limit_total, limit) = if let Some(limit) = request.limit {
            if limit > 0 {
                let limit = std::cmp::min(limit, max_results);
                (std::cmp::min(limit, total_results), limit)
            } else {
                (0, 0)
            }
        } else {
            (std::cmp::min(max_results, total_results), max_results)
        };

        let (has_anchor, anchor) = request
            .anchor
            .map(|anchor| (true, anchor.document_id()))
            .unwrap_or((false, 0));

        QueryResponseBuilder {
            requested_position: request.position.unwrap_or(0),
            position: if has_anchor { 0 } else { request.position.unwrap_or(0) },
            limit: limit_total,
            anchor,
            anchor_offset: request.anchor_offset.unwrap_or(0),
            has_anchor,
            anchor_found: false,
            response: QueryResponse {
                account_id: request.account_id,
                query_state,
                can_calculate_changes: true,
                position: 0,
                ids: vec![],
                total: if request.calculate_total.unwrap_or(false) {
                    Some(total_results)
                } else {
                    None
                },
                limit: if total_results > limit {
                    Some(limit)
                } else {
                    None
                },
            },
        }
    }

    #[inline(always)]
    pub fn add(&mut self, prefix_id: u32, document_id: u32) -> bool {
        self.add_id(Id::from_parts(prefix_id, document_id))
    }

    pub fn add_id(&mut self, id: Id) -> bool {
        let document_id = id.document_id();

        // Pagination
        if !self.has_anchor {
            if self.position >= 0 {
                if self.position > 0 {
                    self.position -= 1;
                } else {
                    self.response.ids.push(id);
                    if self.response.ids.len() == self.limit {
                        return false;
                    }
                }
            } else {
                self.response.ids.push(id);
            }
        } else if self.anchor_offset >= 0 {
            if !self.anchor_found {
                if document_id != self.anchor {
                    self.position += 1;
                    return true;
                }
                self.anchor_found = true;
            }

            if self.anchor_offset > 0 {
                self.anchor_offset -= 1;
                self.position += 1;
            } else {
                self.response.ids.push(id);
                if self.response.ids.len() == self.limit {
                    return false;
                }
            }
        } else {
            self.anchor_found = document_id == self.anchor;
            self.response.ids.push(id);

            if self.anchor_found {
                self.position = self.anchor_offset;
                return false;
            }
        }

        true
    }

    pub fn is_full(&self) -> bool {
        self.response.ids.len() == self.limit
    }

    pub fn build(mut self) -> trc::Result<QueryResponse> {
        if !self.has_anchor || self.anchor_found {
            if !self.has_anchor && self.requested_position >= 0 {
                self.response.position = if self.position == 0 {
                    self.requested_position
                } else {
                    0
                };
            } else if self.position >= 0 {
                self.response.position = self.position;
            } else {
                let position = self.position.unsigned_abs() as usize;
                // anchor_index is the 0-based index of the anchor in the collected ids
                let anchor_index = self.response.ids.len().saturating_sub(1);
                let start_offset = anchor_index.saturating_sub(position);
                self.response.position = start_offset as i32;
                let end_offset = if self.limit > 0 {
                    std::cmp::min(start_offset + self.limit, self.response.ids.len())
                } else {
                    self.response.ids.len()
                };

                self.response.ids = self.response.ids[start_offset..end_offset].to_vec()
            }

            Ok(self.response)
        } else {
            Err(trc::JmapEvent::AnchorNotFound.into_err())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jmap_proto::{method::query::QueryRequest, object::email::Email};

    /// Helper: build a QueryResponseBuilder and feed it document IDs 0..total.
    fn run_query(
        total: usize,
        anchor_doc_id: Option<u32>,
        anchor_offset: Option<i32>,
        position: Option<i32>,
        limit: Option<usize>,
    ) -> (Vec<u32>, i32) {
        let request: QueryRequest<Email> = QueryRequest {
            position,
            anchor: anchor_doc_id.map(|d| Id::from_parts(0, d)),
            anchor_offset,
            limit,
            calculate_total: Some(true),
            ..Default::default()
        };
        let mut builder =
            QueryResponseBuilder::new(total, 10000, State::Initial, &request);
        for doc_id in 0..total as u32 {
            if !builder.add(0, doc_id) {
                break;
            }
        }
        let response = builder.build().unwrap();
        let ids: Vec<u32> = response.ids.iter().map(|id| id.document_id()).collect();
        (ids, response.position)
    }

    // --- Anchor with positive offset (>= 0) ---

    #[test]
    fn anchor_offset_zero() {
        // anchor=10, offset=0, limit=5: return [10,11,12,13,14], position=10
        let (ids, pos) = run_query(20, Some(10), Some(0), None, Some(5));
        assert_eq!(ids, vec![10, 11, 12, 13, 14]);
        assert_eq!(pos, 10);
    }

    #[test]
    fn anchor_offset_positive() {
        // anchor=10, offset=3, limit=5: skip anchor+2 more, return [13,14,15,16,17], position=13
        let (ids, pos) = run_query(20, Some(10), Some(3), None, Some(5));
        assert_eq!(ids, vec![13, 14, 15, 16, 17]);
        assert_eq!(pos, 13);
    }

    #[test]
    fn anchor_offset_zero_unlimited() {
        // anchor=17, offset=0, limit=0: return all from anchor onwards
        let (ids, pos) = run_query(20, Some(17), Some(0), None, Some(0));
        assert_eq!(ids, vec![17, 18, 19]);
        assert_eq!(pos, 17);
    }

    #[test]
    fn anchor_past_end() {
        // anchor=17, offset=5, limit=5: effective position past end, no results
        let (ids, _pos) = run_query(20, Some(17), Some(5), None, Some(5));
        assert!(ids.is_empty());
    }

    // --- Anchor with negative offset ---

    #[test]
    fn anchor_negative_offset() {
        // anchor=10, offset=-5, limit=5: return [5,6,7,8,9], position=5
        // RFC 8620 §5.5: effective_position = 10 + (-5) = 5, take 5 items from index 5
        let (ids, pos) = run_query(20, Some(10), Some(-5), None, Some(5));
        assert_eq!(ids, vec![5, 6, 7, 8, 9]);
        assert_eq!(pos, 5);
    }

    #[test]
    fn anchor_negative_offset_includes_anchor_when_limit_exceeds() {
        // anchor=10, offset=-3, limit=10: effective_position=7, take 10 → [7..17)
        // Items 7,8,9,10,11,12,13,14,15,16 — anchor (10) is included
        let (ids, pos) = run_query(20, Some(10), Some(-3), None, Some(10));
        assert_eq!(ids, vec![7, 8, 9, 10, 11, 12, 13, 14, 15, 16]);
        assert_eq!(pos, 7);
    }

    #[test]
    fn anchor_negative_offset_clamped_to_zero() {
        // anchor=2, offset=-10, limit=5: effective_position = max(0, 2-10) = 0
        let (ids, pos) = run_query(20, Some(2), Some(-10), None, Some(5));
        assert_eq!(ids, vec![0, 1, 2, 3, 4]);
        assert_eq!(pos, 0);
    }

    #[test]
    fn anchor_negative_offset_at_start() {
        // anchor=0, offset=-5, limit=5: effective_position = max(0, -5) = 0
        let (ids, pos) = run_query(20, Some(0), Some(-5), None, Some(5));
        assert_eq!(ids, vec![0, 1, 2, 3, 4]);
        assert_eq!(pos, 0);
    }

    #[test]
    fn anchor_negative_offset_unlimited() {
        // anchor=10, offset=-3, limit=0: effective_position=7, return all from 7 onwards
        let (ids, pos) = run_query(15, Some(10), Some(-3), None, Some(0));
        assert_eq!(ids, vec![7, 8, 9, 10, 11, 12, 13, 14]);
        assert_eq!(pos, 7);
    }

    // --- Anchor not found ---

    #[test]
    fn anchor_not_found() {
        let request: QueryRequest<Email> = QueryRequest {
            anchor: Some(Id::from_parts(0, 999)),
            anchor_offset: Some(0),
            limit: Some(5),
            calculate_total: Some(true),
            ..Default::default()
        };
        let mut builder =
            QueryResponseBuilder::new(10, 10000, State::Initial, &request);
        for doc_id in 0..10u32 {
            builder.add(0, doc_id);
        }
        assert!(builder.build().is_err());
    }

    // --- Position parameter ignored when anchor is set ---

    #[test]
    fn position_ignored_with_anchor() {
        // RFC 8620 §5.5: position is ignored when anchor is set
        let (ids, pos) = run_query(20, Some(10), Some(0), Some(5), Some(3));
        assert_eq!(ids, vec![10, 11, 12]);
        assert_eq!(pos, 10); // Not 5 — position request param is ignored
    }

    // --- Regular pagination (no anchor) ---

    #[test]
    fn regular_pagination() {
        let (ids, pos) = run_query(20, None, None, Some(5), Some(3));
        assert_eq!(ids, vec![5, 6, 7]);
        assert_eq!(pos, 5);
    }
}
