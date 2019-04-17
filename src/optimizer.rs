use crate::filters::network::{NetworkFilter, NetworkFilterMask, FilterPart};
use itertools::*;
use std::collections::HashMap;

trait Optimization {
    fn fusion(&self, filters: &[NetworkFilter]) -> NetworkFilter;
    fn group_by_criteria(&self, filter: &NetworkFilter) -> String;
    fn select(&self, filter: &NetworkFilter) -> bool;
}

/**
 * Fusion a set of `filters` by applying optimizations sequentially.
 */
pub fn optimize(filters: Vec<NetworkFilter>) -> Vec<NetworkFilter> {
    let simple_pattern_group = SimplePatternGroup {};
    let (mut fused, mut unfused) = apply_optimisation(&simple_pattern_group, filters);
    fused.append(&mut unfused);
    fused
}

fn apply_optimisation<T: Optimization>(
    optimization: &T,
    filters: Vec<NetworkFilter>,
) -> (Vec<NetworkFilter>, Vec<NetworkFilter>) {
    let (positive, mut negative): (Vec<NetworkFilter>, Vec<NetworkFilter>) =
        filters.into_iter().partition_map(|f| {
            if optimization.select(&f) {
                Either::Left(f)
            } else {
                Either::Right(f)
            }
        });

    let mut to_fuse: HashMap<String, Vec<NetworkFilter>> = HashMap::with_capacity(positive.len());
    positive
        .into_iter()
        .for_each(|f| insert_dup(&mut to_fuse, optimization.group_by_criteria(&f), f));

    let mut fused = Vec::with_capacity(to_fuse.len());
    for (_, group) in to_fuse {
        if group.len() > 1 {
            // println!("Fusing {} filters together", group.len());
            fused.push(optimization.fusion(group.as_slice()));
        } else {
            group.into_iter().for_each(|f| negative.push(f));
        }
    }

    fused.shrink_to_fit();

    (fused, negative)
}

fn insert_dup<K, V>(map: &mut HashMap<K, Vec<V>>, k: K, v: V)
where
    K: std::cmp::Ord + std::hash::Hash,
{
    map.entry(k).or_insert_with(Vec::new).push(v)
}

struct SimplePatternGroup {}

impl Optimization for SimplePatternGroup {
    // Group simple patterns, into a single filter

    fn fusion(&self, filters: &[NetworkFilter]) -> NetworkFilter {
        let base_filter = &filters[0]; // FIXME: can technically panic, if filters list is empty
        let mut filter = base_filter.clone();

        // if any filter is empty (meaning matches anything), the entire combiation matches anything
        if filters.iter().any(|f| matches!(f.filter, FilterPart::Empty)) {
            filter.filter = FilterPart::Empty
        } else {
            let mut flat_patterns: Vec<String> = Vec::with_capacity(filters.len());
            for f in filters {
                match &f.filter {
                    FilterPart::Empty => (),
                    FilterPart::Simple(s) => flat_patterns.push(s.clone()),
                    FilterPart::AnyOf(s) => flat_patterns.extend_from_slice(s)
                }
            }
            
            if flat_patterns.is_empty() {
                filter.filter = FilterPart::Empty;
            } else if flat_patterns.len() == 1 {
                filter.filter = FilterPart::Simple(flat_patterns[0].clone())
            } else {
                filter.filter = FilterPart::AnyOf(flat_patterns)
            }
        }

        // let is_regex = filters.iter().find(|f| f.is_regex()).is_some();
        filter.mask.set(NetworkFilterMask::IS_REGEX, true);
        let is_complete_regex = filters.iter().any(|f| f.is_complete_regex());
        filter.mask.set(NetworkFilterMask::IS_COMPLETE_REGEX, is_complete_regex);

        if base_filter.raw_line.is_some() {
            filter.raw_line = Some(
                filters
                    .iter()
                    .flat_map(|f| f.raw_line.clone())
                    .join(" <+> "),
            )
        }
        

        filter
    }

    fn group_by_criteria(&self, filter: &NetworkFilter) -> String {
        format!("{:b}:{:?}", filter.mask, filter.is_complete_regex())
    }
    fn select(&self, filter: &NetworkFilter) -> bool {
        !filter.is_fuzzy()
            && filter.opt_domains.is_none()
            && filter.opt_not_domains.is_none()
            && !filter.is_hostname_anchor()
            && !filter.is_redirect()
            && !filter.is_csp()
            && !filter.has_bug()
    }
}

#[cfg(test)]
mod parse_tests {
    use super::*;
    use crate::lists;
    use crate::request::Request;
    use regex::RegexSet;
    use crate::filters::network::CompiledRegex;
    use crate::filters::network::NetworkMatchable;

    fn check_regex_match(regex: &CompiledRegex, pattern: &str, matches: bool) {
        let is_match = regex.is_match(pattern);
        assert!(is_match == matches, "Expected {} match {} = {}", regex.to_string(), pattern, matches);
    }

    #[test]
    fn regex_set_works() {
        let regex_set = RegexSet::new(&[
            r"/static/ad\.",
            "/static/ad-",
            "/static/ad/.*",
            "/static/ads/.*",
            "/static/adv/.*",
        ]);

        let fused_regex = CompiledRegex::CompiledSet(regex_set.unwrap());
        assert!(matches!(fused_regex, CompiledRegex::CompiledSet(_)));
        check_regex_match(&fused_regex, "/static/ad.", true);
        check_regex_match(&fused_regex, "/static/ad-", true);
        check_regex_match(&fused_regex, "/static/ads-", false);
        check_regex_match(&fused_regex, "/static/ad/", true);
        check_regex_match(&fused_regex, "/static/ad", false);
        check_regex_match(&fused_regex, "/static/ad/foobar", true);
        check_regex_match(&fused_regex, "/static/ad/foobar/asd?q=1", true);
        check_regex_match(&fused_regex, "/static/ads/", true);
        check_regex_match(&fused_regex, "/static/ads", false);
        check_regex_match(&fused_regex, "/static/ads/foobar", true);
        check_regex_match(&fused_regex, "/static/ads/foobar/asd?q=1", true);
        check_regex_match(&fused_regex, "/static/adv/", true);
        check_regex_match(&fused_regex, "/static/adv", false);
        check_regex_match(&fused_regex, "/static/adv/foobar", true);
        check_regex_match(&fused_regex, "/static/adv/foobar/asd?q=1", true);
    }

    #[test]
    fn combines_simple_regex_patterns() {
        let rules = vec![
            String::from("/static/ad-"),
            String::from("/static/ad."),
            String::from("/static/ad/*"),
            String::from("/static/ads/*"),
            String::from("/static/adv/*"),
        ];

        let (filters, _) = lists::parse_filters(&rules, true, false, true);

        let optimization = SimplePatternGroup {};

        filters
            .iter()
            .for_each(|f| assert!(optimization.select(f), "Expected rule to be selected"));

        let fused = optimization.fusion(&filters);

        assert!(fused.is_regex(), "Expected rule to be regex");
        assert_eq!(
            fused.to_string(),
            "/static/ad- <+> /static/ad. <+> /static/ad/* <+> /static/ads/* <+> /static/adv/*"
        );

        let fused_regex = fused.get_regex();
        check_regex_match(&fused_regex, "/static/ad-", true);
        check_regex_match(&fused_regex, "/static/ad.", true);
        check_regex_match(&fused_regex, "/static/ad%", false);
        check_regex_match(&fused_regex, "/static/ads-", false);
        check_regex_match(&fused_regex, "/static/ad/", true);
        check_regex_match(&fused_regex, "/static/ad", false);
        check_regex_match(&fused_regex, "/static/ad/foobar", true);
        check_regex_match(&fused_regex, "/static/ad/foobar/asd?q=1", true);
        check_regex_match(&fused_regex, "/static/ads/", true);
        check_regex_match(&fused_regex, "/static/ads", false);
        check_regex_match(&fused_regex, "/static/ads/foobar", true);
        check_regex_match(&fused_regex, "/static/ads/foobar/asd?q=1", true);
        check_regex_match(&fused_regex, "/static/adv/", true);
        check_regex_match(&fused_regex, "/static/adv", false);
        check_regex_match(&fused_regex, "/static/adv/foobar", true);
        check_regex_match(&fused_regex, "/static/adv/foobar/asd?q=1", true);
    }

    #[test]
    fn separates_pattern_by_grouping() {
        let rules = vec![
            String::from("/analytics-v1."),
            String::from("/v1/pixel?"),
            String::from("/api/v1/stat?"),
            String::from("/analytics/v1/*$domain=~my.leadpages.net"),
            String::from("/v1/ads/*"),
        ];

        let (filters, _) = lists::parse_filters(&rules, true, false, true);

        let optimization = SimplePatternGroup {};

        let (fused, skipped) = apply_optimisation(&optimization, filters);

        assert_eq!(fused.len(), 1);
        let filter = fused.get(0).unwrap();
        assert_eq!(
            filter.to_string(),
            "/analytics-v1. <+> /v1/pixel? <+> /api/v1/stat? <+> /v1/ads/*"
        );

        assert!(filter.matches(&Request::from_urls("https://example.com/v1/pixel?", "https://my.leadpages.net", "").unwrap()));

        assert_eq!(skipped.len(), 1);
        let filter = skipped.get(0).unwrap();
        assert_eq!(
            filter.to_string(),
            "/analytics/v1/*$domain=~my.leadpages.net"
        );

        assert!(filter.matches(&Request::from_urls("https://example.com/analytics/v1/foobar", "https://foo.leadpages.net", "").unwrap()))
    }

}
