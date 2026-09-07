use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    iter::{empty, once},
    rc::Rc,
    sync::{Arc, LazyLock},
};

use gc_value::{GcValue, Map as GcMap};
use serde_json::Value as JsonValue;
use strum::{EnumIter, FromRepr, IntoEnumIterator};
use uuid::Uuid;

// algos ====================================================================
// reference: https://www.w3.org/TR/2026/WD-cbor-ld-10-20260902/

/// §6.1.1 <https://www.w3.org/TR/cbor-ld-10/#json-ld-to-cbor-ld-encoding-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - instead of returning `cborldBytes`, we write it directly into `writer`
/// - it takes an extra parameter context_cache
pub fn jsonld_to_cborld<W: Write>(
    type_table: TypeTable,
    registry_entry_id: RegistryEntryId,
    jsonld_document: JsonValue,
    mut writer: W,
    context_cache: impl IntoIterator<Item = (String, JsonValue)>,
) -> Result<()> {
    // step 1 (NB: we do not retrieve `prefix`, instead it is written directly into `writer`)
    get_cbor_tag_structure(registry_entry_id, &mut writer)?;

    // step 2 skipped (state will be initialized later)

    // necessary plumbing
    let Some(entry) = get_registry_entry(registry_entry_id) else {
        return Err(Error::UnsupportedEntry);
    };

    // step 3 (NB: we do not retrieve `suffix`, instead it is written directly into `writer`)
    if entry.processing_model.compression {
        let state = State::new(Strategy::Compression, entry, type_table, context_cache);
        // NB: initialize_conversion_algorithm is managed inside `State::new`.
        let output = convert_document(state, serde_json::from_value(jsonld_document)?)?;
        serde_cbor::to_writer(writer, &output)?;
    // step 4 (NB: we do not retrieve `suffix`, instead it is written directly into `writer`)
    } else {
        serde_cbor::to_writer(writer, &jsonld_document)?;
    };

    // step 5-6 skipped (as a result of writing directly to `writer` in the previous steps)
    Ok(())
}

/// §6.1.2 <https://www.w3.org/TR/cbor-ld-10/#cbor-ld-to-json-ld-decoding-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - it takes an extra parameter context_cache
pub fn cborld_to_jsonld<R: Read>(
    cborld_bytes: R,
    context_cache: impl IntoIterator<Item = (String, JsonValue)>,
) -> Result<JsonValue> {
    // step 1-3
    let (suffix, registry_entry_id) = get_registry_entry_id(cborld_bytes)?;

    // necessary plumbing
    let Some(entry) = get_registry_entry(registry_entry_id) else {
        return Err(Error::UnsupportedEntry);
    };

    // step 4
    let jsonld_document = if entry.processing_model.compression {
        // step 4.2
        let type_table = entry.type_tables.clone();
        // NB: state.reverse_type_table is set in State::new

        // step 4.3
        let state = State::new(Strategy::Decompression, entry, type_table, context_cache);
        // NB: initialize_conversion_algorithm is managed inside `State::new`.

        // step 4.4-4.5
        let jsonld_document_gc = convert_document(state, GcValue::Map(suffix))?;
        serde_json::to_value(jsonld_document_gc)?
    // step 5
    } else {
        serde_json::to_value(GcValue::Map(suffix))?
    };

    // step 6
    Ok(jsonld_document)
}

#[allow(dead_code)]
/// §6.2.1 <https://www.w3.org/TR/cbor-ld-10/#initialize-conversion-algorithm>
///
/// NB: this algorithm is actually integrated in [`State::new`],
/// so this function is a noop, and is actually not used in this implementation.
pub fn initialize_conversion(state: State) -> State {
    state
}

/// §6.2.2 <https://www.w3.org/TR/cbor-ld-10/#convert-document-algorithm>
///
///
/// NB: the signature does not match the current signature from the spec (2026-07-02)
/// but matches how the algorithm is actually used in other algorithms.
pub fn convert_document(mut state: State, input_document: GcValue) -> Result<GcValue> {
    // step 1-5
    match input_document {
        GcValue::Array(maps) => {
            let res_vec: Result<Vec<_>> = maps
                .iter()
                .map(|map| {
                    let GcValue::Map(input) = map else {
                        return Err(Error::InvalidPayloadStructure);
                    };
                    let active_context = state.initial_active_context.clone();
                    general_conversion(input, &mut state, active_context)
                })
                .collect();
            res_vec.map(GcValue::new_array)
        }
        GcValue::Map(input) => {
            let active_context = state.initial_active_context.clone();
            general_conversion(&input, &mut state, active_context)
        }
        _ => Err(Error::InvalidPayloadStructure),
    }
}

/// §6.2.3 <https://www.w3.org/TR/cbor-ld/#general-conversion-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - `state` is passed as a mutable reference, rather than returned back
/// - `output` parameter is not passed (it is always an empty map)
pub fn general_conversion(
    input: &GcMap,
    state: &mut State,
    active_context: Rc<ActiveContext>,
) -> Result<GcValue> {
    let compression = state.strategy == Strategy::Compression;

    // step 1
    let (mut output, active_context) = if compression {
        convert_context_for_compression(state, active_context, input)?
    // step 2
    } else {
        convert_context_for_decompression(state, active_context, input)?
    };

    // step 3
    let object_types = if compression {
        get_object_types_for_compression(&active_context, input)
    // step 4
    } else {
        get_object_types_for_decompression(state, &active_context, input)?
    };

    // step 5
    let active_context = apply_type_scoped_contexts(state, active_context, object_types)?;

    // step 6
    let term_entries = if compression {
        get_input_entries_for_compression(state, &active_context, input)
    // step 7
    } else {
        get_input_entries_for_decompression(state, &active_context, input)?
    };

    // step 8
    for (term_info, value) in term_entries {
        // step 8.1
        let term = &term_info.term;

        // step 8.2
        let value_active_context =
            apply_property_scoped_contexts(state, active_context.clone(), term)?;

        // step 8.3
        let plural = term_info.plural;
        let term_type = term_info.get_from_def("@type");

        // step 8.4 - 8.7
        let output_values = if plural {
            let GcValue::Array(v) = &value else {
                unreachable!("expect an array in value when plural is true");
            };
            v.iter()
                .map(|unconverted_value| {
                    convert_value(
                        state,
                        value_active_context.clone(),
                        &term_info,
                        unconverted_value,
                        term_type,
                    )
                })
                .collect::<Result<GcValue>>()?
        } else {
            convert_value(state, active_context.clone(), &term_info, value, term_type)?
        };

        // step 8.8
        let key = if compression {
            term_info.get_term_id()
        } else {
            GcValue::Text(term_info.term.clone())
        };
        output.insert(key, output_values);
    }

    // step 9-11
    Ok(GcValue::collect_map(output))
}

/// §6.2.4 <https://www.w3.org/TR/cbor-ld/#convert-value-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - `state` is passed as a mutable reference, rather than returned back
pub fn convert_value(
    state: &mut State,
    active_context: Rc<ActiveContext>,
    term_info: &TermInfo,
    value: &GcValue,
    term_type: Option<&GcValue>,
) -> Result<GcValue> {
    debug_assert!(term_type.map(GcValue::is_json).unwrap_or(true));

    // step 1
    if matches!(value, GcValue::Null) {
        return Ok(GcValue::Null);
    }

    // step 2
    let output = if state.strategy == Strategy::Compression {
        convert_value_for_compression(state, term_info, value, term_type)?
    // step 3
    } else {
        convert_value_for_decompression(state, term_info, term_type, value)?
    };

    // step 4
    if let Some(output) = output {
        return Ok(output);
    }

    Ok(match value {
        // step 5
        GcValue::Array(elements) => elements
            .iter()
            .map(|element| {
                convert_value(state, active_context.clone(), term_info, element, term_type)
            })
            .collect::<Result<GcValue>>()?,
        // step 6-8
        GcValue::Map(input) => general_conversion(input, state, active_context)?,
        _ => unreachable!(),
    })
}

/// §6.2.5.1 <https://www.w3.org/TR/cbor-ld/#convert-contexts-for-compression-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - `state` is passed as a mutable reference, rather than returned back
/// - `output` parameter is not passed (it is always an empty map)
pub fn convert_context_for_compression(
    state: &mut State,
    active_context: Rc<ActiveContext>,
    input: &GcMap,
) -> Result<(BTreeMap<GcValue, GcValue>, Rc<ActiveContext>)> {
    let mut output = BTreeMap::new();

    // steps 1-2
    let active_context = apply_embedded_contexts(state, active_context, input)?;

    // step 3-4
    let Some(context) = input.get("@context") else {
        return Ok((output, active_context));
    };

    // step 5-9
    //
    // closure for steps 7.1-7.3
    let make_encoded_context = |context_value: &GcValue| {
        if let Some(encoder_data) = create_context_encoder(&state.type_table, context_value) {
            // NB: encode_context is not called explicitly in the spec,
            // but is meant to be invoked later on on the 'context encoder'
            encode_context(encoder_data)
        } else {
            context_value.clone()
        }
    };
    match context {
        GcValue::Array(contexts) => {
            let encoded_contexts: GcValue = contexts.iter().map(make_encoded_context).collect();
            // step 8
            debug_assert!(state.keywords_map.get("@context") == Some(&0));
            output.insert(1.into(), encoded_contexts);
        }
        context_value => {
            let encoded_context = make_encoded_context(context_value);
            // step 9
            debug_assert!(state.keywords_map.get("@context") == Some(&0));
            output.insert(0.into(), encoded_context);
        }
    }

    // step 10-11
    Ok((output, active_context))
}

/// §6.2.5.2 <https://www.w3.org/TR/cbor-ld/#convert-value-for-compression-algorithm>
///
pub fn convert_value_for_compression(
    state: &State,
    term_info: &TermInfo,
    value_to_encode: &GcValue,
    term_type: Option<&GcValue>,
) -> Result<Option<GcValue>> {
    debug_assert!(term_type.map(GcValue::is_json).unwrap_or(true));

    match value_to_encode {
        // not in the spec, but seems to be implied
        GcValue::Array(_) => Ok(None),
        // step 1
        GcValue::Map(_) => Ok(None),
        // step 2
        _ => Ok(Some(create_value_encoder(
            state,
            term_info,
            term_type,
            value_to_encode,
        )?)),
    }
}

/// §6.2.5.3 <https://www.w3.org/TR/cbor-ld/#get-input-entries-for-compression-algorithm>
///
/// Pre-condition: `input` comes from a JSON document, so all its keys are excpeted to be strings
pub fn get_input_entries_for_compression<'a>(
    state: &State,
    active_context: &ActiveContext,
    input: &'a GcMap,
) -> Vec<TermEntry<'a>> {
    // step 2 (retrieving items instead of just keys)
    let mut items: Vec<_> = input
        .iter()
        .map(|(k, v)| match k {
            GcValue::Text(s) => (s, v),
            _ => unreachable!(),
        })
        .collect();
    items.sort_unstable_by_key(|(k, _)| *k);

    // step 3-4
    items
        .into_iter()
        // step 3.1
        .filter(|(key, _)| key.as_ref() != "@context")
        .map(|(key, value)| {
            let plural = matches!(value, GcValue::Array(_));
            // step 3.4-3.6
            // (noting that, in step 3.4, we store None rather than `key`)
            let term_id = state
                .term_to_id
                .get(key)
                .map(|v| if plural { *v + 1 } else { *v });
            // step 3.7
            let definition = active_context.term_map.get(key).cloned();
            // step 3.8-3.11
            (
                TermInfo {
                    term: key.clone(),
                    term_id,
                    plural,
                    def: definition,
                },
                value,
            )
        })
        .collect()
}

/// §6.2.5.4 <https://www.w3.org/TR/cbor-ld/#get-object-types-for-compression-algorithm>
///
pub fn get_object_types_for_compression(
    active_context: &ActiveContext,
    input: &GcMap,
) -> BTreeSet<Arc<str>> {
    active_context
        .type_terms
        .iter()
        .filter_map(|term| input.get(term))
        .flat_map(|types| -> Box<dyn Iterator<Item = _>> {
            match types {
                GcValue::Array(v) => Box::new(v.iter().filter_map(GcValue::as_text)),
                GcValue::Text(s) => Box::new(std::iter::once(s)),
                _ => Box::new(empty()),
            }
        })
        .cloned()
        .collect()
}

/// §6.2.6.1 <https://www.w3.org/TR/cbor-ld/#convert-contexts-for-decompression-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - `state` is passed as a mutable reference, rather than returned back
/// - `output` parameter is not passed (it is always an empty map)
pub fn convert_context_for_decompression(
    state: &mut State,
    active_context: Rc<ActiveContext>,
    input: &GcMap,
) -> Result<(BTreeMap<GcValue, GcValue>, Rc<ActiveContext>)> {
    let mut output = BTreeMap::new();

    // step 1
    let decoder_data = create_context_decoder(&state.reverse_type_table);

    // step 2
    debug_assert!(state.keywords_map.get("@context") == Some(&0));

    // step 3
    let opt_encoded_context = input.get(&0);
    if let Some(encoded_context) = opt_encoded_context {
        let context = decode_context(&decoder_data, encoded_context)?;
        output.insert("@context".into(), context);
    }

    // step 4
    // contextTermIdPlural is always 1 (see step 2)

    // step 5
    if let Some(encoded_contexts) = input.get(&1) {
        // step 5.1
        if opt_encoded_context.is_some() {
            return Err(Error::InvalidEncodedContext);
        }
        // step 5.2
        let GcValue::Array(encoded_contexts) = encoded_contexts else {
            return Err(Error::InvalidEncodedContext);
        };
        // step 5.3-5.4
        let contexts: GcValue = encoded_contexts
            .iter()
            .map(|value_to_decode| decode_context(&decoder_data, value_to_decode))
            .collect::<Result<GcValue>>()?;
        // step 5.5
        output.insert("@context".into(), contexts);
    }

    // step 6
    let output_as_map: GcMap = output.clone().into_iter().collect();
    let active_context = apply_embedded_contexts(state, active_context, &output_as_map)?;

    // step 7-9
    Ok((output, active_context))
}

/// §6.2.6.2 <https://www.w3.org/TR/cbor-ld/#convert-value-for-decompression-algorithm>
///
pub fn convert_value_for_decompression(
    state: &State,
    term_info: &TermInfo,
    term_type: Option<&GcValue>,
    value_to_decode: &GcValue,
) -> Result<Option<GcValue>> {
    debug_assert!(term_type.map(GcValue::is_json).unwrap_or(true));

    match value_to_decode {
        // step 1
        GcValue::Map(_) => Ok(None),
        // step 2-3
        _ => Ok(create_value_decoder(
            state,
            term_info,
            term_type,
            value_to_decode,
        )?),
    }
}

/// §6.2.6.3 <https://www.w3.org/TR/cbor-ld/#get-input-entries-for-decompression-algorithm>
///
pub fn get_input_entries_for_decompression<'a>(
    state: &State,
    active_context: &ActiveContext,
    input: &'a GcMap,
) -> Result<Vec<TermEntry<'a>>> {
    debug_assert!(state.keywords_map.get("@context") == Some(&0));
    // step 1-2
    let mut entries = input
        .iter()
        // step 2.1
        .filter(|(k, _)| k.as_int().map(|k| k > 1).unwrap_or(true))
        .map(|(key, value)| {
            let (plural, term, term_id) = match key {
                // step 2.2
                GcValue::Text(term) => (false, term, None),
                // step 2.3
                GcValue::Int(key) => {
                    let key = *key as usize;
                    let (plural, term_id) = if key % 2 == 1 {
                        (true, key - 1)
                    } else {
                        (false, key)
                    };
                    let term = state
                        .id_to_term
                        .get(&term_id)
                        .ok_or(Error::UnknownCborldTerm)?;
                    (plural, term, Some(term_id))
                }
                // step 2.3bis (not in the spec, but should be)
                _ => return Err(Error::InvalidEncodedKey),
            };
            // step 2.4
            let definition = active_context.term_map.get(term).cloned();
            // steps 2.5-2.8
            Ok((
                TermInfo {
                    term: term.clone(),
                    term_id,
                    plural,
                    def: definition,
                },
                value,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    // step 3
    entries.sort_unstable_by(|e1, e2| e1.0.term.cmp(&e2.0.term));
    // step 4
    Ok(entries)
}

/// §6.2.6.4 <https://www.w3.org/TR/cbor-ld/#get-object-types-for-decompression-algorithm>
///
/// This implementation currently differs from the spec,
/// applying my proposal in <https://github.com/w3c/cbor-ld/issues/107>
pub fn get_object_types_for_decompression(
    state: &State,
    active_context: &ActiveContext,
    input: &GcMap,
) -> Result<BTreeSet<Arc<str>>> {
    // step 1 is implicit (captured by the collect below)

    active_context
        .type_terms
        .iter()
        .flat_map(|term| -> Box<dyn Iterator<Item = _>> {
            // steps 2.1-2.2 (setting term_id to None instead of term)
            let term_id = state.term_to_id.get(term).copied();

            // steps 2.3-2.7 (with changes suggested in #107)
            let Some((value, plural)) = (match &term_id {
                None => input.get(term).map(|v| (v, false)),
                Some(n) => {
                    let n = *n as u64;
                    input
                        .get(&n)
                        .map(|v| (v, false))
                        .or_else(|| input.get(&(n + 1)).map(|v| (v, true)))
                }
            }) else {
                return Box::new(empty());
            };

            // step 2.8
            let definition = active_context.term_map.get(term).cloned();

            // steps 2.9-2.10
            let term_info = TermInfo {
                term: term.clone(),
                term_id,
                plural,
                def: definition,
            };

            // step 2.11
            let values: Box<dyn Iterator<Item = _>> = match value {
                GcValue::Array(values) => Box::new(values.iter()),
                _ => Box::new(once(value)),
            };

            // step 2.12
            Box::new(values.filter_map(move |value| {
                // step 2.12.1
                create_value_decoder(state, &term_info, Some(&"@vocab".to_string().into()), value)
                    .map(|opt| match opt {
                        // step 2.12.2
                        Some(decoded) => decoded,
                        // step 2.12.3
                        None => value.clone(),
                    })
                    .map(|decoded_value|
                    // not in the spec: if value is not a string, ignore it.
                    // This should be equivalent to the spec,
                    // because any non-string added to objectTypes will fail to match,
                    // in step 4.1 of 5.3.4 Apply Type Scoped Contexts
                    match decoded_value {
                        GcValue::Text(s) => Some(s),
                        _ => None,
                    })
                    .transpose()
            }))
        })
        .collect()

    // step 3 is implicit (returning the result of collect above)
}

/// §6.3.1 <https://www.w3.org/TR/cbor-ld/#initialize-active-context-algorithm>
///
pub fn initialize_active_context(
    previous_active_context: Option<Rc<ActiveContext>>,
    term_map: TermMap,
) -> Rc<ActiveContext> {
    let type_terms = [Arc::from("@type")]
        .into_iter()
        .chain(term_map.iter().filter_map(|(term, def)| {
            def.get("@id")
                .filter(|v| *v == "@type")
                .map(|_| term.clone())
        }))
        .collect();
    Rc::new(ActiveContext {
        previous_active_context,
        term_map,
        type_terms,
    })
}

/// §6.3.2 <https://www.w3.org/TR/cbor-ld/#apply-embedded-contexts-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - `state` is passed as a mutable reference, rather than returned back
pub fn apply_embedded_contexts(
    state: &mut State,
    active_context: Rc<ActiveContext>,
    input: &GcMap,
) -> Result<Rc<ActiveContext>> {
    // not currently in the spec
    let Some(contexts) = input.get("@context") else {
        return Ok(active_context);
    };
    if !contexts.is_json() {
        log::warn!("ignoring embedded @context that is not a valid JSON object");
        return Ok(active_context);
    }

    // steps 1-3
    let active_term_map = update_term_map(state, &active_context.term_map, contexts, false, false)?;

    // step 4-6
    Ok(initialize_active_context(
        Some(active_context),
        active_term_map,
    ))
}

/// §6.3.3 <https://www.w3.org/TR/cbor-ld/#apply-property-scoped-contexts-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - `state` is passed as a mutable reference, rather than returned back
pub fn apply_property_scoped_contexts(
    state: &mut State,
    active_context: Rc<ActiveContext>,
    term: &str,
) -> Result<Rc<ActiveContext>> {
    // step 1
    let reverted_term_map = revert_term_map(&active_context);

    // step 2
    let opt_context = active_context
        .term_map
        .get(term)
        .and_then(|term_def| term_def.get("@context"));

    // not currently in the spec
    let Some(contexts) = opt_context else {
        return Ok(active_context);
    };

    // steps 3-5
    let term_map = update_term_map(state, &reverted_term_map, contexts, false, true)?;

    // steps 6-8
    Ok(initialize_active_context(Some(active_context), term_map))
}

/// §6.3.4 <https://www.w3.org/TR/cbor-ld/#apply-type-scoped-contexts-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - `state` is passed as a mutable reference, rather than returned back
pub fn apply_type_scoped_contexts(
    state: &mut State,
    active_context: Rc<ActiveContext>,
    object_types: BTreeSet<Arc<str>>,
) -> Result<Rc<ActiveContext>> {
    // steps 1-2 not needed, object_types is already sorted

    // step 3
    let mut new_term_map = active_context.term_map.clone();

    // step 4
    for typ in object_types {
        // step 4.1
        let opt_context = new_term_map
            .get(&typ)
            .and_then(|type_def| type_def.get("@context"));

        // not currently in the spec
        let Some(contexts) = opt_context else {
            continue;
        };

        // step 4.2-4.4
        new_term_map = update_term_map(state, &new_term_map, contexts, true, false)?;
    }

    // steps 5-7
    Ok(initialize_active_context(
        Some(active_context),
        new_term_map,
    ))
}

/// §6.3.5 <https://www.w3.org/TR/cbor-ld/#update-term-map-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - `state` is passed as a mutable reference, rather than returned back
/// - the key `propagate` in TermMap can not contain a
/// - in step 4.5.3, the condition has been reversed ('term' DOES NOT appear on 'context), which seems the correct thing to do
pub fn update_term_map(
    state: &mut State,
    active_term_map: &TermMap,
    contexts: &GcValue,
    type_scope: bool,
    property_scope: bool,
) -> Result<TermMap> {
    debug_assert!(contexts.is_json());

    // step 1
    let contexts: Box<dyn Iterator<Item = _>> = match contexts {
        GcValue::Array(v) => Box::new(v.iter()),
        _ => Box::new(once(contexts)),
    };

    // step 2
    let allow_protected_override = property_scope;

    // step 3
    let propagate_default = !type_scope;

    // step 4
    let mut active_term_map: Cow<TermMap> = Cow::Borrowed(active_term_map);
    for context_identifier in contexts {
        // steps 4.1-4.2
        let entry = load_context(state, context_identifier)?;
        let context = &entry.context;
        // step 4.3
        let propagate = context
            .get("@propagate")
            .and_then(GcValue::as_boolean)
            .unwrap_or(propagate_default);
        // step 4.4
        let mut new_term_map: BTreeMap<_, _> = entry
            .term_map
            .iter()
            .map(|(key, value)| (key.clone(), value.with("propagate", propagate)))
            .collect();
        // step 4.5
        for (term, active_def) in active_term_map.iter() {
            // step 4.5.1
            if let Some(def) = new_term_map.get(term) {
                // step 4.5.2
                if active_def
                    .get("protected")
                    .and_then(GcValue::as_boolean)
                    .unwrap_or(false)
                {
                    if !allow_protected_override && def != active_def {
                        return Err(Error::ProtectedTermRedefinition(term.clone()));
                    } else {
                        new_term_map.insert(
                            term.clone(),
                            if let Some(propagate) = def.get("propagate") {
                                active_def.with("propagate", propagate.clone())
                            } else {
                                active_def.without("propagate")
                            },
                        );
                    }
                }
            } else if context.get(term).is_none() {
                // step 4.5.3
                new_term_map.insert(term.clone(), active_def.clone());
            }
        }
        // step 4.6
        active_term_map = Cow::Owned(new_term_map);
    }
    // steps 5-7
    Ok(active_term_map.into_owned())
}

/// §6.3.6 <https://www.w3.org/TR/cbor-ld/#revert-term-map-algorithm>
///
pub fn revert_term_map(active_context: &ActiveContext) -> TermMap {
    // step 1
    let mut new_term_map = BTreeMap::new();

    // step 2
    let mut non_propagating_terms = Vec::with_capacity(active_context.term_map.len());

    // step 3
    for (term, def) in &active_context.term_map {
        if def.get("propagate").and_then(GcValue::as_boolean) == Some(false) {
            non_propagating_terms.push(term);
        } else {
            new_term_map.insert(term.clone(), def.clone());
        }
    }

    // step 4
    for term in non_propagating_terms {
        let mut current_context = Some(active_context);
        let mut def;
        loop {
            current_context =
                current_context.and_then(|c| c.previous_active_context.as_ref().map(Rc::as_ref));
            def = current_context.and_then(|c| c.term_map.get(term));
            if let Some(d) = def
                && d.get("propagate").and_then(GcValue::as_boolean) == Some(false)
            {
                continue;
            } else {
                break;
            }
        }
        if let Some(def) = def {
            new_term_map.insert(term.clone(), def.clone());
        }
    }

    // step 5
    new_term_map
}

#[allow(dead_code)]
/// §6.4.1 <https://www.w3.org/TR/cbor-ld-10/#initialize-context-loader-algorithm>
///
/// NB: this algorithm is actually integrated in [`State::new`],
/// so this function is a noop, and is actually not used in this implementation.
pub fn initialize_context_loader(state: State) -> State {
    state
}

/// §6.4.2 <https://www.w3.org/TR/cbor-ld/#load-context-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - `state` is passed as a mutable reference, rather than returned back
/// - it raises errors if fetch fails, if parsing the result to json fails,
///   if context_identifier is neither a string nor a map
///
/// Pre-condition: context_identifier is either a string or a map
pub fn load_context(state: &mut State, context_identifier: &GcValue) -> Result<Rc<ContextEntry>> {
    let (context, context_url) = if let Some(uri) = context_identifier.as_text() {
        // step 1
        if let Some(entry) = state.context_map.get(context_identifier) {
            return Ok(entry.clone());
        }
        // step 2
        let context = fetch_context(state, uri)?;
        (context, Some(uri.clone()))
    // step 3
    } else if let GcValue::Map(map) = context_identifier {
        (map.clone(), None)
    // not in the spec
    } else {
        return Err(Error::InvalidContext);
    };
    // step 4
    add_context(state, context, context_url)
}

/// §6.4.3 <https://www.w3.org/TR/cbor-ld/#add-context-algorithm>
///
/// This implementations differs from the algorithm in the spec:
/// - `state` is passed as a mutable reference, rather than returned back
pub fn add_context(
    state: &mut State,
    mut context: Arc<GcMap>,
    context_url: Option<Arc<str>>,
) -> Result<Rc<ContextEntry>> {
    // step 1
    if let Some(import) = context.get("@import")
        && let Some(import_url) = import.as_text()
    {
        let mut map = BTreeMap::from_iter(context.iter().cloned());
        let import_entry = match state.context_map.get(import) {
            // step 1.2
            None => {
                let import_context = fetch_context(state, import_url)?;
                add_context(state, import_context, Some(import_url.clone()))?
            }
            // step 1.3
            Some(entry) => entry.clone(),
        };
        // step 1.4
        for (k, v) in import_entry.context.iter() {
            map.entry(k.clone()).or_insert(v.clone());
        }
        context = Arc::new(GcMap::from_iter(map));
    }

    // step 2
    let mut term_map = BTreeMap::new();

    // step 3 (building ContextEntry 'entry' is postponed to please the borrow checker)

    // step 4 (nothing to do, keys are already sorted in BTreeMap)

    // step 5
    let is_protected = context.get("@protected").is_some();

    // step 6
    for (term, definition) in context.iter() {
        let term = term.as_text().unwrap();

        // step 6.1
        if state.keywords_map.contains_key(term) {
            continue;
        }
        // step 6.2: definition is already set
        let mut definition = match definition {
            // step 6.3
            GcValue::Null => continue,
            // step 6.4
            GcValue::Text(s) => Arc::new(GcMap::from_iter([("@id", s.clone())])),
            // not in the spec
            GcValue::Map(map) => map.clone(),
            _ => {
                log::warn!("ignoring invalid term definition for {term:?}");
                continue;
            }
        };
        // step 6.5
        definition = definition.with("protected", is_protected);
        // step 6.6
        term_map.insert(term.clone(), definition);
        // step 6.7
        let term: Arc<str> = term.clone();
        state
            .term_to_id
            .entry(term.clone())
            .or_insert_with_key(|key| {
                let term_id = state.next_term_id;
                state.next_term_id += 2;
                state.id_to_term.insert(term_id, key.clone());
                term_id
            });
    }

    // delayed step 3
    let entry = Rc::new(ContextEntry { context, term_map });

    // step 7
    if let Some(context_url) = context_url {
        state.context_map.insert(context_url.into(), entry.clone());
    // step 8
    } else {
        state
            .context_map
            .insert(entry.context.clone().into(), entry.clone());
    }

    // steps 9-11
    Ok(entry)
}

/// §6.5.1.1 <https://www.w3.org/TR/cbor-ld/#create-context-encoder>
///
/// This implementations differs from the algorithm in the spec:
/// - it returns "an empty map" (i.e. None) if type_table.context is undefined
pub fn create_context_encoder<'a>(
    type_table: &'a TypeTable,
    context_value: &'a GcValue,
) -> Option<ContextEncoder<'a>> {
    // step 1-2
    let context_url = context_value.as_text()?.as_ref();

    // step 3
    let context_table = type_table.get("context")?;

    // steps 4-5
    Some(ContextEncoder {
        context: context_url,
        context_table,
    })
}

/// §6.5.1.2 <https://www.w3.org/TR/cbor-ld/#encode-context>
///
/// This implementations differs from the algorithm in the spec:
/// it does not produce binary data, but the GcValue that will be serialized as expected by the spec
pub fn encode_context(encoder_data: ContextEncoder<'_>) -> GcValue {
    if let Some(n) = encoder_data.context_table.get(encoder_data.context) {
        GcValue::new_int(*n as i128)
    } else {
        GcValue::new_text(encoder_data.context)
    }
}

/// §6.5.1.3 <https://www.w3.org/TR/cbor-ld/#create-context-decoder>
///
pub fn create_context_decoder<'a>(reverse_type_table: &'a ReverseTypeTable) -> ContextDecoder<'a> {
    let reverse_context_table = reverse_type_table.get("context");
    ContextDecoder {
        reverse_context_table,
    }
}

/// §6.5.1.4 <https://www.w3.org/TR/cbor-ld/#decode-context>
///
pub fn decode_context(decoder_data: &ContextDecoder<'_>, value: &GcValue) -> Result<GcValue> {
    let GcValue::Int(n) = value else {
        // step 1
        return Ok(value.clone());
    };

    // step 2
    if let Some(entry) = decoder_data
        .reverse_context_table
        .and_then(|map| map.get(&(*n as usize)))
    {
        Ok(GcValue::new_text(entry.clone()))
    } else {
        Err(Error::UndefinedCompressedContext)
    }
}

/// §6.5.2.1 <https://www.w3.org/TR/cbor-ld/#create-value-encoder>
///
/// This implementations differs from the algorithm in the spec:
/// - instead of returning a map `value_encoder`, it returns the encoded value directly
pub fn create_value_encoder(
    state: &State,
    term_info: &TermInfo,
    term_type: Option<&GcValue>,
    value_to_encode: &GcValue,
) -> Result<GcValue> {
    debug_assert!(term_type.map(GcValue::is_json).unwrap_or(true));

    // not in the spec, but seems to be implied
    let term_type = term_type.and_then(GcValue::as_text);

    // steps 1-4
    let is_url = id_or_type(&term_info.term)
        || term_info
            .get_from_def("@id")
            .and_then(GcValue::as_text)
            .map(id_or_type)
            .unwrap_or(false)
        || term_type
            .map(|t| t.as_ref() == "@id" || t.as_ref() == "@vocab")
            .unwrap_or(false);

    // steps 5-7
    let table_type = if is_url {
        "url"
    } else {
        term_type.map(AsRef::as_ref).unwrap_or("none")
    };

    // step 8
    if let Some(sub_table) = state.type_table.get(table_type) {
        // step 8.2 + 8.4
        if let Some(key) = value_to_encode.as_text()
            && let Some(int_value) = sub_table.get(key)
        {
            let int_value = *int_value as i128;
            let include_sign = true;
            let convert_to_bytes = state.types_encoded_as_bytes.contains(table_type);
            return Ok(encode_value(int_value, include_sign, convert_to_bytes));
        // step 8.3 + 8.4
        } else if table_type != "none"
            && let Some(int_value) = value_to_encode.as_int()
        {
            let include_sign = true;
            let convert_to_bytes = true;
            return Ok(encode_value(int_value, include_sign, convert_to_bytes));
        }
    }

    // step 9 interpreting freely the spec (state.processingModeTypeEncoders is actually not defined)
    let value_encoder = if state.codecs.contains(table_type) {
        match table_type {
            "url" => create_url_encoder(state, term_info, value_to_encode)?,
            _ => return Err(Error::UnknownCodec),
        }
    } else {
        None
    };

    // step 10
    if let Some(encoded_value) = value_encoder {
        Ok(encoded_value)
    // step 11
    } else {
        // part of the way the internal representation is encoded in CBOR
        if let Some(f) = value_to_encode.as_float() {
            if f >= 0.0 && f <= MAX_SAFE_INT && f.fract() == 0.0 {
                return Ok((f as i128).into());
            }
            if f < 0.0 && -f <= MAX_SAFE_INT && f.fract() == 0.0 {
                return Ok((f as i128).into());
            }
        } else if let Some(i) = value_to_encode.as_int()
            && (i > MAX_SAFE_INT as i128 || i < -MAX_SAFE_INT as i128)
        {
            return Ok((i as f64).into());
        }
        Ok(value_to_encode.clone())
    }
}

/// §6.5.2.2 <https://www.w3.org/TR/cbor-ld/#encode-value>
///
/// This implementations differs from the algorithm in the spec:
/// - instead of a map, it expects the values of the map directly
/// - it does not produce binary data, but the GcValue that will be serialized as expected by the spec
pub fn encode_value(int_value: i128, include_sign: bool, convert_to_bytes: bool) -> GcValue {
    // step 1
    if convert_to_bytes {
        GcValue::new_bytes(int_to_bytes(int_value, include_sign))
    // step 2
    } else {
        GcValue::Int(int_value)
    }
}

/// §6.5.2.3 <https://www.w3.org/TR/cbor-ld/#create-value-decoder>
///
/// This implementations differs from the algorithm in the spec:
/// - instead of returning a map `decoderData`, it returns the decoded value directly
pub fn create_value_decoder(
    state: &State,
    term_info: &TermInfo,
    term_type: Option<&GcValue>,
    value_to_decode: &GcValue,
) -> Result<Option<GcValue>> {
    debug_assert!(term_type.map(GcValue::is_json).unwrap_or(true));

    // not in the spec, but seems to be implied
    let term_type = term_type.and_then(GcValue::as_text);

    // steps 1-4
    let is_url = id_or_type(&term_info.term)
        || term_info
            .get_from_def("@id")
            .and_then(GcValue::as_text)
            .map(id_or_type)
            .unwrap_or(false)
        || term_type
            .map(|t| t.as_ref() == "@id" || t.as_ref() == "@vocab")
            .unwrap_or(false);

    // steps 5-7
    let table_type = if is_url {
        "url"
    } else {
        term_type.map(AsRef::as_ref).unwrap_or("none")
    };

    // step 8
    if let Some(sub_table) = state.reverse_type_table.get(table_type) {
        let (use_table, int_value) = match value_to_decode {
            // step 8.3
            GcValue::Bytes(bytes) if bytes.len() <= 8 => (true, bytes_to_int(bytes, false)),
            // step 8.4
            GcValue::Int(int_value) => (true, *int_value),
            // implied by 8.2
            _ => (false, 0),
        };
        // step 8.5 + 8.7
        if use_table {
            let Some(decoded) = sub_table.get(&(int_value as usize)) else {
                return Err(Error::UnknownCompressedValue);
            };
            let decoded = GcValue::new_text(decoded.clone());
            return Ok(Some(decoded));
        // step 8.6 + 8.7
        } else if let GcValue::Bytes(bytes) = value_to_decode
            && bytes.len() <= 8
            && table_type != "none"
        {
            // not in the spec: test above that bytes.len() <= 8
            let decoded = GcValue::Int(bytes_to_int(bytes, true));
            return Ok(Some(decoded));
        }
    }

    // step 9 interpreting freely the spec (state.processingModeTypeEncoders is actually not defined)
    let decoder_data = if state.codecs.contains(table_type) {
        match table_type {
            "url" => create_url_decoder(state, term_info, value_to_decode)?,
            _ => return Err(Error::UnknownCodec),
        }
    } else {
        None
    };

    // step 10
    if let Some(decoded_value) = decoder_data {
        Ok(Some(decoded_value))
    // step 11
    } else if !value_to_decode.is_array() {
        Ok(Some(value_to_decode.clone()))
    // not specified but seems implies
    } else {
        Ok(None)
    }
}

// §6.5.3.1 <https://www.w3.org/TR/cbor-ld-10/#url-prefix-table>
// encoded slightly differently
#[repr(u64)]
#[derive(Clone, Copy, Debug, EnumIter, FromRepr)]
pub enum UrlSubCodec {
    Http = 1,
    Https = 2,
    UrnUuid = 3,
    Data = 4,
    DidV1Nym = 1024,
    DidKey = 1025,
}

impl UrlSubCodec {
    fn prefix(&self) -> &'static str {
        use UrlSubCodec::*;
        match self {
            Http => "http://",
            Https => "https://",
            UrnUuid => "urn:uuid:",
            Data => "data:",
            DidV1Nym => "did:v1:nym:",
            DidKey => "did:key",
        }
    }

    fn from_i128(id: i128) -> Option<Self> {
        Self::from_repr(id as u64)
    }

    fn from_url(url: &str) -> Option<(Self, &'static str)> {
        Self::iter()
            .map(|id| (id, id.prefix()))
            .find(|(_, prefix)| url.starts_with(prefix) && !url[prefix.len()..].contains(":"))
    }
}

/// §6.5.3.2 <https://www.w3.org/TR/cbor-ld-10/#create-url-encoder>
///
/// This implementations differs from the algorithm in the spec:
/// - instead of returning a map `value_encoder`, it returns the encoded value directly
pub fn create_url_encoder(
    state: &State,
    _term_info: &TermInfo,
    value_to_encode: &GcValue,
) -> Result<Option<GcValue>> {
    // step 1
    let Some(value_to_encode) = value_to_encode.as_text() else {
        return Ok(None);
    };

    // steps 2-3
    if let Some(id) = state.term_to_id.get(value_to_encode) {
        return Ok(Some((*id).into()));
    }

    // step 4
    if reqwest::Url::parse(value_to_encode).is_err() {
        return Ok(None);
    }

    // step 5
    let Some((id, prefix)) = UrlSubCodec::from_url(value_to_encode) else {
        // step 10
        return Ok(None);
    };

    // steps 6-9
    use UrlSubCodec::*;
    match id {
        Http | Https =>
        // §6.5.3.6.1 <https://www.w3.org/TR/cbor-ld-10/#create-http-url-encoder>
        {
            Ok(Some(GcValue::new_array([
                GcValue::new_int(id as u64),
                GcValue::new_text(&value_to_encode[prefix.len()..]),
            ])))
        }
        UrnUuid => {
            // §6.5.3.7.1 <https://www.w3.org/TR/cbor-ld-10/#create-uuid-urn-encoder>
            let rest = &value_to_encode[prefix.len()..];
            let suffix = if rest.bytes().all(|b| !b.is_ascii_uppercase()) {
                // NB: do *not* use b.is_ascii_lowercase() above, as digits and dash are *not* lowercase
                let uuid = Uuid::try_parse(rest)?;
                GcValue::new_bytes(&uuid.as_bytes()[..])
            } else {
                GcValue::new_text(rest)
            };
            Ok(Some(GcValue::new_array([
                GcValue::new_int(id as u64),
                suffix,
            ])))
        }
        Data => {
            // §6.5.3.8.1 <https://www.w3.org/TR/cbor-ld-10/#create-data-url-encoder>
            let rest = &value_to_encode[prefix.len()..];
            if let Some((mediatype, data_bytes)) = || -> Option<_> {
                let pos = rest.rfind(";base64,")?;
                let mediatype = &rest[..pos];
                let data = &rest[pos + 8..];
                let data_bytes = BASE64.decode(data.as_bytes()).ok()?;
                if BASE64.encode(&data_bytes) != data {
                    // non-canonical encoding; fallback to encoding it as text
                    return None;
                }
                Some((mediatype, data_bytes))
            }() {
                Ok(Some(GcValue::new_array([
                    GcValue::new_int(id as u64),
                    GcValue::new_text(mediatype),
                    GcValue::new_bytes(data_bytes),
                ])))
            } else {
                Ok(Some(GcValue::new_array([
                    GcValue::new_int(id as u64),
                    GcValue::new_text(rest),
                ])))
            }
        }
        DidV1Nym => todo!(),
        DidKey => todo!(),
    }
}

/// §6.5.3.4 <https://www.w3.org/TR/cbor-ld-10/#create-url-decoder>
///
/// This implementations differs from the algorithm in the spec:
/// - instead of returning a decoder object, it returns the decoded value directly
pub fn create_url_decoder(
    state: &State,
    _term_info: &TermInfo,
    value_to_decode: &GcValue,
) -> Result<Option<GcValue>> {
    Ok(match value_to_decode {
        // step 1
        GcValue::Text(_) => None,
        // Not clearly specified, although it might be inferred from step 2.7
        GcValue::Array(a) if a.is_empty() || !a[0].is_int() => Err(Error::UnknownCompressedValue)?,
        // step 2
        GcValue::Array(a) => {
            // step 2.1
            let [GcValue::Int(prefix_id), rest @ ..] = &a[..] else {
                unreachable!() // see above
            };
            // step 2.2
            let Some(prefix_id) = UrlSubCodec::from_i128(*prefix_id) else {
                // step 2.7
                Err(Error::UnknownCompressedValue)?
            };
            use UrlSubCodec::*;
            match prefix_id {
                // step 2.3
                Http | Https => {
                    // §6.5.3.6.3 <https://www.w3.org/TR/cbor-ld-10/#create-http-url-decoder>
                    // step 1
                    if let [GcValue::Text(suffix)] = rest {
                        let prefix = prefix_id.prefix();
                        Some(format!("{prefix}{suffix}").into())
                    // step 2
                    } else {
                        None
                    }
                }
                // step 2.4
                UrnUuid => {
                    // §6.5.3.7.3 <https://www.w3.org/TR/cbor-ld/#create-uuid-urn-decoder>
                    match rest {
                        // step 1
                        [GcValue::Text(suffix)] => {
                            let prefix = prefix_id.prefix();
                            Some(format!("{prefix}{suffix}").into())
                        }
                        // step 2
                        [GcValue::Bytes(bytes)] => uuid::Builder::from_slice(bytes)
                            .ok()
                            .map(|b| b.into_uuid().urn().to_string().into()),
                        _ => None,
                    }
                }
                // step 2.5
                Data => {
                    // §6.5.3.8.3 <https://www.w3.org/TR/cbor-ld/#create-data-url-decoder>
                    let prefix = prefix_id.prefix();
                    match rest {
                        [GcValue::Text(rest)] => Some(format!("{prefix}{rest}").into()),
                        [GcValue::Text(mediatype), GcValue::Bytes(data)] => {
                            let data_encoded = BASE64.encode(data);
                            Some(format!("{prefix}{mediatype};base64,{data_encoded}").into())
                        }
                        _ => None,
                    }
                }
                // step 2.6
                DidV1Nym | DidKey => todo!(),
            }
        }
        // step 3
        GcValue::Int(i) => state
            .id_to_term
            .get(&(*i as usize))
            .map(|t| GcValue::new_text(t.clone())),
        // step 4
        _ => None,
    })
}

/// §6.6.1 <https://www.w3.org/TR/cbor-ld-10/#get-cbor-tag-structure-algorithm>
///
///
/// Instead of returning `prefix`, we write it into `writer`
pub fn get_cbor_tag_structure<W: Write>(
    registry_entry_id: RegistryEntryId,
    mut writer: W,
) -> serde_cbor::Result<()> {
    writer.write_all(b"\xD9\xCB\x1D\x82")?;
    serde_cbor::to_writer(&mut writer, &registry_entry_id)?;
    Ok(())
}

/// §6.6.2 <https://www.w3.org/TR/cbor-ld-10/#get-registry-entry-id-algorithm>
///
pub fn get_registry_entry_id<R: Read>(
    mut cborld_bytes: R,
) -> Result<(Arc<GcMap>, RegistryEntryId)> {
    // step 1
    let mut header = [0_u8; 3];
    cborld_bytes.read_exact(&mut header[..])?;
    if &header != b"\xd9\xcb\x1d" {
        return Err(Error::NonCborLdTag);
    }
    let item: GcValue = serde_cbor::from_reader(cborld_bytes)?;
    // step 2
    let GcValue::Array(vec) = item else {
        return Err(Error::InvalidPayloadStructure);
    };
    let [GcValue::Int(raw_id), GcValue::Map(suffix)] = &vec[..] else {
        return Err(Error::InvalidPayloadStructure);
    };
    let Ok(registry_entry_id): Result<RegistryEntryId, _> = (*raw_id).try_into() else {
        return Err(Error::InvalidPayloadStructure);
    };
    // step 3-7
    Ok((suffix.clone(), registry_entry_id))
}

pub fn get_registry_entry<'a>(registry_entry_id: RegistryEntryId) -> Option<&'a RegistryEntry> {
    static REGISTRY: LazyLock<Vec<RegistryEntry>> = LazyLock::new(|| {
        vec![
            RegistryEntry {
                id: 0,
                use_case: "Uncompressed CBOR-LD".into(),
                type_tables: BTreeMap::new(),
                processing_model: ProcessingModel {
                    compression: false,
                    codecs: BTreeSet::new(),
                },
                provisional: false,
            },
            RegistryEntry {
                id: 1,
                use_case: "Compressed CBOR-LD".into(),
                type_tables: BTreeMap::new(),
                processing_model: ProcessingModel {
                    compression: true,
                    codecs: BTreeSet::from(["url".to_string()]),
                },
                provisional: false,
            },
            RegistryEntry {
                id: 100,
                use_case: "Verifiable Credential Barcodes Examples".into(),
                type_tables: vec![
                    (
                        "context".into(),
                        [
                            ("https://www.w3.org/ns/credentials/v2".into(), 32768),
                            ("https://w3id.org/vc-barcodes/v1".into(), 32769),
                            ("https://w3id.org/utopia/v2".into(), 32770),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                    (
                        "https://w3id.org/security#cryptosuiteString".into(),
                        [
                            ("ecdsa-rdfc-2019".into(), 1),
                            ("ecdsa-sd-2023".into(), 2),
                            ("eddsa-rdfc-2022".into(), 3),
                            ("ecdsa-xi-2023".into(), 4),
                        ]
                        .into_iter()
                        .collect(),
                    ),
                ]
                .into_iter()
                .collect(),
                processing_model: ProcessingModel {
                    compression: true,
                    codecs: BTreeSet::new(),
                },
                provisional: true,
            },
        ]
    });
    let i = REGISTRY
        .binary_search_by_key(&registry_entry_id, |e| e.id)
        .ok()?;
    Some(&REGISTRY[i])
}

// utility functions ========================================================

fn reverse_map<K: Clone, V: Clone + Ord>(map: &BTreeMap<K, V>) -> BTreeMap<V, K> {
    map.iter().map(|(k, v)| (v.clone(), k.clone())).collect()
}

/// Fetch context from cache or from the web.
///
/// Return a `GcMap` that is JSON-compatible.
fn fetch_context(state: &mut State, uri: &str) -> Result<Arc<GcMap>> {
    if let Some(cached) = state.context_cache.get(uri) {
        return Ok(cached.clone());
    }
    let body = reqwest::blocking::get(uri)?.error_for_status()?.text()?;
    let jsonld_doc: GcValue = serde_json::from_str(&body)?;
    let GcValue::Map(map) = jsonld_doc else {
        return Err(Error::InvalidContext);
    };
    map.get("@context")
        .inspect(|v| debug_assert!(v.is_json()))
        .and_then(GcValue::as_map)
        .cloned()
        .ok_or(Error::InvalidContext)
}

fn id_or_type<T: AsRef<str>>(txt: T) -> bool {
    txt.as_ref() == "@id" || txt.as_ref() == "@type"
}

// TODO check with Wes about this
fn bytes_to_int(b: &[u8], signed: bool) -> i128 {
    debug_assert!(b.len() <= 8);
    let mut a = [0_u8; 8];
    a[b.len() - 8..].copy_from_slice(b);
    if signed {
        i64::from_le_bytes(a) as i128
    } else {
        u64::from_le_bytes(a) as i128
    }
}

// TODO check with Wes about this
fn int_to_bytes(i: i128, signed: bool) -> Vec<u8> {
    if signed {
        (i as i64).to_le_bytes().to_vec()
    } else {
        (i as u64).to_le_bytes().to_vec()
    }
}

// types ====================================================================

#[derive(Clone, Debug)]
pub struct State {
    strategy: Strategy,
    type_table: TypeTable,
    reverse_type_table: ReverseTypeTable,
    #[expect(unused)] // TODO why is is set in the spec, then??
    registry_entry_id: RegistryEntryId,
    // TODO not defined in the spec
    codecs: BTreeSet<String>,
    initial_active_context: Rc<ActiveContext>,
    types_encoded_as_bytes: BTreeSet<String>,
    context_map: BTreeMap<GcValue, Rc<ContextEntry>>,
    next_term_id: usize,
    keywords_map: BTreeMap<Arc<str>, usize>,
    term_to_id: BTreeMap<Arc<str>, usize>,
    id_to_term: BTreeMap<usize, Arc<str>>,
    context_cache: BTreeMap<Arc<str>, Arc<GcMap>>,
}

impl State {
    pub fn new(
        strategy: Strategy,
        entry: &RegistryEntry,
        type_table: TypeTable,
        context_cache: impl IntoIterator<Item = (String, JsonValue)>,
    ) -> Self {
        let registry_entry_id = entry.id;
        let codecs = entry.processing_model.codecs.clone();
        let initial_active_context = Rc::new(ActiveContext::default());
        let types_encoded_as_bytes = [
            "none",
            "http://www.w3.org/2001/XMLSchema#date",
            "http://www.w3.org/2001/XMLSchema#dateTime",
            "url",
        ]
        .map(String::from)
        .into();
        let context_map = BTreeMap::new();
        let next_term_id = 100;
        let keywords_map: BTreeMap<Arc<str>, usize> = [
            ("@context", 0),
            ("@type", 2),
            ("@id", 4),
            ("@value", 6),
            ("@direction", 8),
            ("@graph", 10),
            ("@included", 12),
            ("@index", 14),
            ("@json", 16),
            ("@language", 18),
            ("@list", 20),
            ("@nest", 22),
            ("@reverse", 24),
            ("@base", 26),
            ("@container", 28),
            ("@default", 30),
            ("@embed", 32),
            ("@explicit", 34),
            ("@none", 36),
            ("@omitDefault", 38),
            ("@prefix", 40),
            ("@preserve", 42),
            ("@protected", 44),
            ("@requireAll", 46),
            ("@set", 48),
            ("@version", 50),
            ("@vocab", 52),
            ("@propagate", 54),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect();
        let term_to_id = keywords_map.clone();
        let (id_to_term, reverse_type_table) = if strategy == Strategy::Compression {
            (BTreeMap::new(), BTreeMap::new())
        } else {
            (
                reverse_map(&term_to_id),
                type_table
                    .iter()
                    .map(|(k, submap)| (k.clone(), reverse_map(submap)))
                    .collect(),
            )
        };
        let context_cache = context_cache
            .into_iter()
            .map(|(k, v)| {
                (
                    Arc::from(k),
                    serde_json::from_value::<GcValue>(v)
                        .unwrap()
                        .to_map()
                        .unwrap(),
                )
            })
            .collect();
        Self {
            strategy,
            type_table,
            reverse_type_table,
            registry_entry_id,
            codecs,
            initial_active_context,
            types_encoded_as_bytes,
            context_map,
            next_term_id,
            keywords_map,
            term_to_id,
            id_to_term,
            context_cache,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strategy {
    Compression,
    Decompression,
}

pub type TypeTable = BTreeMap<Arc<str>, BTreeMap<Arc<str>, usize>>;
pub type ReverseTypeTable = BTreeMap<Arc<str>, BTreeMap<usize, Arc<str>>>;

#[derive(Clone, Debug)]
pub struct ActiveContext {
    previous_active_context: Option<Rc<ActiveContext>>,
    term_map: TermMap,
    type_terms: Vec<Arc<str>>,
}

impl Default for ActiveContext {
    fn default() -> Self {
        let previous_active_context = None;
        let term_map = TermMap::new();
        let type_terms = vec![Arc::from("@type")];
        ActiveContext {
            previous_active_context,
            term_map,
            type_terms,
        }
    }
}

type TermMap = BTreeMap<Arc<str>, Arc<GcMap>>;

#[derive(Clone, Debug)]
pub struct TermInfo {
    term: Arc<str>,
    /// Differ from spec: instead of a string (= term), we store None
    term_id: Option<usize>,
    plural: bool,
    /// Differ from spec: instead of an empty map, we store None
    def: Option<Arc<GcMap>>,
}

impl TermInfo {
    fn get_term_id(&self) -> GcValue {
        if let Some(n) = self.term_id {
            GcValue::Int(n as i128)
        } else {
            GcValue::Text(self.term.clone())
        }
    }

    fn get_from_def(&self, key: &str) -> Option<&GcValue> {
        self.def.as_ref().and_then(|map| map.get(key))
    }
}

type TermEntry<'a> = (TermInfo, &'a GcValue);

type RegistryEntryId = u64;

#[derive(Clone, Debug)]
pub struct RegistryEntry {
    pub id: RegistryEntryId,
    pub use_case: String,
    pub type_tables: TypeTable,
    pub processing_model: ProcessingModel,
    pub provisional: bool,
}

#[derive(Clone, Debug)]
pub struct ProcessingModel {
    pub compression: bool,
    pub codecs: BTreeSet<Codec>,
}

#[derive(Clone, Debug)]
pub struct ContextEncoder<'a> {
    pub context: &'a str,
    pub context_table: &'a BTreeMap<Arc<str>, usize>,
}

#[derive(Clone, Debug)]
pub struct ContextDecoder<'a> {
    pub reverse_context_table: Option<&'a BTreeMap<usize, Arc<str>>>,
}

pub struct ValueEncoder(Box<dyn FnOnce() -> GcValue>);

impl ValueEncoder {
    pub fn new(int_value: i128, include_sign: bool, convert_to_bytes: bool) -> ValueEncoder {
        (move || encode_value(int_value, include_sign, convert_to_bytes)).into()
    }

    pub fn encode(self) -> GcValue {
        (self.0)()
    }
}

impl<F: FnOnce() -> GcValue + 'static> From<F> for ValueEncoder {
    fn from(value: F) -> Self {
        Self(Box::new(value))
    }
}

impl From<GcValue> for ValueEncoder {
    fn from(value: GcValue) -> Self {
        Self(Box::new(|| value))
    }
}

// This type is implicitly present in the spec
// (as a map containing only the 'decoded' key)
// but it seems not very interesting...
//
// All value decoders do not even have the same interface
// (the generic one )
// TODO: check with Wes whether it could be simplified
#[derive(Clone, Debug)]
pub struct ValueDecoder {
    pub decoded: GcValue,
}

#[derive(Clone, Debug)]
pub struct ContextEntry {
    context: Arc<GcMap>,
    term_map: TermMap,
}

pub type Codec = String;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("X_ERR_INVALID_CONTEXT")]
    InvalidContext,
    #[error("ERR_INVALID_ENCODED_CONTEXT")]
    InvalidEncodedContext,
    #[error("X_ERR_INVALID_ENCODED_KEY")]
    InvalidEncodedKey,
    #[error("ERR_INVALID_PAYLOAD_STRUCTURE")]
    InvalidPayloadStructure,
    #[error("ERR_NON_CBOR_LD_TAG")]
    NonCborLdTag,
    #[error("ERR_PROTECTED_TERM_REDEFINITION")]
    ProtectedTermRedefinition(Arc<str>),
    #[error("ERR_UNKNOWN_CBORLD_TERM")]
    UnknownCborldTerm,
    #[error("X_ERR_UNKNOWN_CODEC")]
    UnknownCodec,
    #[error("ERR_UNDEFINED_COMPRESSED_CONTEXT")]
    UndefinedCompressedContext,
    #[error("ERR_UNKNOWN_COMPRESSED_VALUE")]
    UnknownCompressedValue,
    #[error("X_ERR_UNSUPPORTED_ENTRY")]
    UnsupportedEntry,

    #[error("Reqwest: {0}")]
    Reqwest(
        #[from]
        #[source]
        reqwest::Error,
    ),
    #[error("IO: {0}")]
    Io(
        #[from]
        #[source]
        std::io::Error,
    ),
    #[error("CBOR (De)Serialization: {0}")]
    SerdeCbor(
        #[from]
        #[source]
        serde_cbor::Error,
    ),
    #[error("JSON (De)Serialization: {0}")]
    SerdeJson(
        #[from]
        #[source]
        serde_json::Error,
    ),
    #[error("Uuid {0}")]
    Uuid(
        #[from]
        #[source]
        uuid::Error,
    ),
}

type Result<T, E = Error> = std::result::Result<T, E>;

static BASE64: LazyLock<data_encoding::Encoding> = LazyLock::new(|| {
    let mut spec = data_encoding::Specification::new();
    spec.symbols
        .push_str("ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/");
    spec.padding = Some('=');
    spec.check_trailing_bits = false;
    spec.encoding().unwrap()
});

static MAX_SAFE_INT: f64 = 9_007_199_254_740_991.0;

// types ====================================================================

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, fs::File, path::Path};

    use super::*;
    use test_case::test_case;

    include!(concat!(env!("OUT_DIR"), "/tests.in"));

    // called by tests.in above
    fn run_decode(path: &str) -> TestResult {
        println!("{path}");
        let cborld = File::open(Path::new(path).with_extension("cborld"))?;
        let got = cborld_to_jsonld(cborld, OUT_DIR_CONTEXT_CACHE.iter().cloned())?;
        let jsonld = File::open(Path::new(path).with_extension("jsonld"))?;
        let exp: JsonValue = serde_json::from_reader(jsonld)?;
        assert_eq!(got, exp);
        Ok(())
    }
    // called by tests.in above
    fn run_encode(path: &str) -> TestResult {
        println!("{path}");
        let jsonld = File::open(Path::new(path).with_extension("jsonld"))?;
        let jsonld = serde_json::from_reader(jsonld)?;
        let mut got: Vec<u8> = vec![];
        jsonld_to_cborld(
            Default::default(),
            1,
            jsonld,
            &mut got,
            OUT_DIR_CONTEXT_CACHE.iter().cloned(),
        )?;
        let mut cborld = File::open(Path::new(path).with_extension("cborld"))?;
        let mut exp: Vec<u8> = vec![];
        cborld.read_to_end(&mut exp)?;
        if got != exp {
            let got: GcValue = serde_cbor::from_slice(&got)?;
            let exp: GcValue = serde_cbor::from_slice(&exp)?;
            assert_eq!(got, exp);
        }
        Ok(())
    }

    static OUT_DIR_CONTEXT_CACHE: LazyLock<Vec<(String, JsonValue)>> = LazyLock::new(|| {
        let ctx = Path::new(env!("OUT_DIR")).join("tests").join("ctx");
        let mut ret = vec![];
        for res in std::fs::read_dir(ctx).unwrap() {
            let path = res.unwrap().path();
            if path.extension() == Some(OsStr::new("jsonld")) {
                let url = format!(
                    "http://localhost:8000/ctx/{}",
                    path.file_name().unwrap().to_string_lossy()
                );
                let value: JsonValue = serde_json::from_reader(File::open(path).unwrap()).unwrap();
                ret.push((
                    url,
                    value.as_object().unwrap().get("@context").unwrap().clone(),
                ));
            }
        }
        ret
    });

    #[test_case("1.0", "1"; "small positive int as float")]
    #[test_case("-1.0", "-1"; "small negative int as float")]
    #[test_case("9e15", "9000000000000000"; "big positive int as float")]
    #[test_case("-9e15", "-9000000000000000"; "big negative int as float")]
    #[test_case("9007199254740992", "9007199254740992.0"; "unsafe positive integer float as int")]
    #[test_case("-9007199254740992", "-9007199254740992.0"; "unsafe negative integer as int")]
    #[test_case("1000000000000000000000", "1e21"; "big integer float as int")]
    fn equiv(json1: &str, json2: &str) -> TestResult {
        let json1 = serde_json::from_slice(format!(r#"{{ "x:p": {json1} }}"#).as_bytes())?;
        let json2 = serde_json::from_slice(format!(r#"{{ "x:p": {json2} }}"#).as_bytes())?;

        let mut cbor1: Vec<u8> = vec![];
        jsonld_to_cborld(Default::default(), 1, json1, &mut cbor1, [])?;
        let mut cbor2: Vec<u8> = vec![];
        jsonld_to_cborld(Default::default(), 1, json2, &mut cbor2, [])?;

        if cbor1 != cbor2 {
            let cbor1: GcValue = serde_cbor::from_slice(&cbor1)?;
            let cbor2: GcValue = serde_cbor::from_slice(&cbor2)?;
            assert_eq!(cbor1, cbor2);
        }

        Ok(())
    }

    type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;
}
