//! Group Space pagination counts RootSpace while preserving unconsumed service rows.
use super::common::map_ids_error;
use crate::domain::{
    client::Client,
    types::{ListMyGroupSpacesInput, ListMyGroupSpacesOutput, SpaceInfo},
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tos_core::agent::error::CliError;

const CURSOR_NAMESPACE: &str = "adrive-group-spaces:";
const CURSOR_PREFIX: &str = "adrive-group-spaces:v1:";
const MAX_PAGE_SIZE: i32 = 1000;
const MAX_LISTING_PAGES: usize = 10_000;

pub(super) struct GroupSpaces {
    pub spaces: Vec<SpaceInfo>,
    pub root_space: Option<SpaceInfo>,
    pub root_id: Option<String>,
    pub next_marker: Option<String>,
    pub request_id: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    instance: String,
    marker: Option<String>,
    offset: usize,
    limit: Option<i32>,
    root_id: Option<String>,
    cycle: Cycle,
}

// Brent cycle detection retains constant-sized state across separate CLI invocations.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cycle {
    checkpoint: Option<String>,
    power: u64,
    distance: u64,
}

impl Cycle {
    fn advance(&mut self, marker: &str) -> Result<(), CliError> {
        self.distance += 1;
        if self.checkpoint.as_deref() == Some(marker) {
            return Err(invalid("service returned cyclic group Space pagination"));
        }
        if self.distance == self.power {
            self.checkpoint = Some(marker.into());
            self.power = self.power.checked_mul(2).unwrap_or(self.power);
            self.distance = 0;
        }
        Ok(())
    }
}

impl Cursor {
    fn decode(instance: &str, marker: Option<&str>) -> Result<Self, CliError> {
        let marker = marker.filter(|value| !value.is_empty());
        let Some(encoded) = marker.and_then(|value| value.strip_prefix(CURSOR_PREFIX)) else {
            reject_scoped_cursor(marker)?;
            return Ok(Self {
                instance: instance.into(),
                marker: marker.map(str::to_owned),
                offset: 0,
                limit: None,
                root_id: None,
                cycle: Cycle {
                    checkpoint: marker.map(str::to_owned),
                    power: 1,
                    distance: 0,
                },
            });
        };
        let bytes = hex::decode(encoded).map_err(|_| invalid("malformed CLI group cursor"))?;
        let cursor: Self =
            serde_json::from_slice(&bytes).map_err(|_| invalid("malformed CLI group cursor"))?;
        if cursor.instance != instance {
            return Err(invalid("CLI group cursor belongs to another Instance"));
        }
        if cursor
            .limit
            .is_some_and(|limit| !(1..=MAX_PAGE_SIZE).contains(&limit))
            || (cursor.offset > 0 && cursor.limit.is_none())
            || cursor.root_id.as_ref().is_some_and(|id| id.is_empty())
            || cursor.marker.as_ref().is_some_and(|value| value.is_empty())
            || !cursor.cycle.power.is_power_of_two()
            || cursor.cycle.distance >= cursor.cycle.power
        {
            return Err(invalid("invalid CLI group cursor state"));
        }
        Ok(cursor)
    }

    fn encode(&self) -> Result<String, CliError> {
        Ok(format!(
            "{CURSOR_PREFIX}{}",
            hex::encode(serde_json::to_vec(self)?)
        ))
    }
}

/// Reject a CLI group cursor on another owner collection; raw service markers remain accepted.
/// Returns a validation error before a cursor can be sent to the wrong service route.
pub(super) fn reject_scoped_cursor(marker: Option<&str>) -> Result<(), CliError> {
    if marker.is_some_and(|value| value.starts_with(CURSOR_NAMESPACE)) {
        return Err(invalid("CLI group cursor requires the same Instance and --owner-type group; unsupported cursor versions cannot be used"));
    }
    Ok(())
}

/// List the group collection with RootSpace included in the result cap and continuation state.
/// Propagates service errors and rejects invalid, cross-instance or cyclic pagination.
pub(super) async fn list(
    client: &Client,
    instance: &str,
    max_keys: i32,
    marker: Option<&str>,
) -> Result<GroupSpaces, CliError> {
    let mut listing = Listing::new(instance, max_keys, marker)?;
    loop {
        let input = listing.next_request()?;
        let page = client
            .list_my_group_spaces(&input)
            .await
            .map_err(map_ids_error)?;
        if listing.accept(page, input.limit.unwrap_or(MAX_PAGE_SIZE))? {
            return listing.finish();
        }
    }
}

struct Listing {
    cursor: Cursor,
    max_keys: usize,
    spaces: Vec<SpaceInfo>,
    space_ids: HashSet<String>,
    root_space: Option<SpaceInfo>,
    markers: HashSet<Option<String>>,
    request_id: String,
}

impl Listing {
    fn new(instance: &str, max_keys: i32, marker: Option<&str>) -> Result<Self, CliError> {
        if max_keys <= 0 {
            return Err(invalid("--max-keys must be positive"));
        }
        Ok(Self {
            cursor: Cursor::decode(instance, marker)?,
            max_keys: max_keys as usize,
            spaces: Vec::new(),
            space_ids: HashSet::new(),
            root_space: None,
            markers: HashSet::new(),
            request_id: String::new(),
        })
    }

    fn next_request(&mut self) -> Result<ListMyGroupSpacesInput, CliError> {
        if self.markers.len() >= MAX_LISTING_PAGES {
            return Err(invalid(
                "group Space listing exceeded the 10000-page safety bound",
            ));
        }
        if !self.markers.insert(self.cursor.marker.clone()) {
            return Err(invalid("service returned cyclic group Space pagination"));
        }
        // [Review Fix #GroupSpaces1] Reserve root's slot without consuming an ordinary row beyond the cap.
        let reserve = usize::from(self.cursor.root_id.is_none());
        let remaining = self.max_keys - self.spaces.len();
        let limit = self.cursor.limit.unwrap_or(
            remaining
                .saturating_sub(reserve)
                .clamp(1, MAX_PAGE_SIZE as usize) as i32,
        );
        let mut input = ListMyGroupSpacesInput::new(&self.cursor.instance).with_limit(limit);
        input.marker = self.cursor.marker.clone();
        Ok(input)
    }

    fn accept(&mut self, page: ListMyGroupSpacesOutput, limit: i32) -> Result<bool, CliError> {
        self.request_id = page.response_info.request_id().into();
        if !page.next_marker.is_empty() && self.markers.contains(&Some(page.next_marker.clone())) {
            return Err(invalid("service returned cyclic group Space pagination"));
        }
        if self.cursor.offset > page.spaces.len() {
            return Err(invalid(
                "group Space page changed; restart listing without --marker",
            ));
        }
        self.accept_root(page.root_space, &page.spaces);
        let offset = self.cursor.offset;
        for (index, space) in page.spaces.into_iter().enumerate().skip(offset) {
            if self.is_duplicate(&space) {
                continue;
            }
            if self.spaces.len() == self.max_keys {
                // [Review Fix #GroupSpaces2] Replay the original request before advancing past any unreturned Space.
                self.cursor.offset = index;
                self.cursor.limit = Some(limit);
                return Ok(true);
            }
            self.space_ids.insert(space.space_id.clone());
            self.spaces.push(space);
        }
        if !page.next_marker.is_empty() {
            // [Review Fix #GroupSpaces5] Carry cycle detection across logical pages without an ever-growing marker history.
            self.cursor.cycle.advance(&page.next_marker)?;
        }
        self.cursor.marker = (!page.next_marker.is_empty()).then_some(page.next_marker);
        self.cursor.offset = 0;
        self.cursor.limit = None;
        Ok(self.cursor.marker.is_none() || self.spaces.len() == self.max_keys)
    }

    fn accept_root(&mut self, root: Option<SpaceInfo>, page: &[SpaceInfo]) {
        self.prefer_complete_root(page);
        let Some(root) = root else { return };
        if self
            .root_space
            .as_ref()
            .is_none_or(|value| value.space_id.is_empty())
        {
            self.root_space = Some(root.clone());
        }
        if root.space_id.is_empty() {
            return;
        }
        if self.cursor.root_id.is_some() {
            return;
        }
        self.cursor.root_id = Some(root.space_id.clone());
        let existing = self
            .spaces
            .iter()
            .position(|space| space.space_id == root.space_id);
        let existing = existing.map(|index| self.spaces.remove(index));
        let mut root = existing
            .map(|record| complete_root(record, &root))
            .unwrap_or(root);
        for record in page
            .iter()
            .filter(|record| Some(&record.space_id) == self.cursor.root_id.as_ref())
        {
            root = complete_root(record.clone(), &root);
        }
        self.space_ids.insert(root.space_id.clone());
        self.spaces.insert(0, root);
    }

    fn prefer_complete_root(&mut self, page: &[SpaceInfo]) {
        let Some(root_id) = &self.cursor.root_id else {
            return;
        };
        // [Review Fix #GroupSpaces4] A later page can enrich root even when it omits the side field.
        if let Some(existing) = self
            .spaces
            .first_mut()
            .filter(|space| &space.space_id == root_id)
        {
            for complete in page.iter().filter(|space| &space.space_id == root_id) {
                *existing = complete_root(complete.clone(), existing);
            }
        }
    }

    fn is_duplicate(&self, space: &SpaceInfo) -> bool {
        self.cursor.root_id.as_ref() == Some(&space.space_id)
            || self.space_ids.contains(&space.space_id)
    }

    fn finish(self) -> Result<GroupSpaces, CliError> {
        let has_more = self.cursor.limit.is_some() || self.cursor.marker.is_some();
        let next_marker = has_more.then(|| self.cursor.encode()).transpose()?;
        Ok(GroupSpaces {
            spaces: self.spaces,
            root_space: self.root_space,
            root_id: self.cursor.root_id,
            next_marker,
            request_id: self.request_id,
        })
    }
}

fn complete_root(mut record: SpaceInfo, fallback: &SpaceInfo) -> SpaceInfo {
    // [Review Fix #GroupSpaces6] Prefer the normal Space record while preserving fields omitted from it.
    for (field, value) in [
        (&mut record.instance_id, &fallback.instance_id),
        (&mut record.name, &fallback.name),
        (&mut record.display_name, &fallback.display_name),
        (&mut record.description, &fallback.description),
        (&mut record.owner_type, &fallback.owner_type),
        (&mut record.owner_id, &fallback.owner_id),
    ] {
        if field.is_empty() {
            field.clone_from(value);
        }
    }
    if record.created_at == 0 {
        record.created_at = fallback.created_at;
    }
    if record.updated_at == 0 {
        record.updated_at = fallback.updated_at;
    }
    record
}

fn invalid(message: &str) -> CliError {
    CliError::ValidationError(message.into())
}
