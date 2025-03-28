// ┏━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┓
// ┃ ██████ ██████ ██████       █      █      █      █      █ █▄  ▀███ █       ┃
// ┃ ▄▄▄▄▄█ █▄▄▄▄▄ ▄▄▄▄▄█  ▀▀▀▀▀█▀▀▀▀▀ █ ▀▀▀▀▀█ ████████▌▐███ ███▄  ▀█ █ ▀▀▀▀▀ ┃
// ┃ █▀▀▀▀▀ █▀▀▀▀▀ █▀██▀▀ ▄▄▄▄▄ █ ▄▄▄▄▄█ ▄▄▄▄▄█ ████████▌▐███ █████▄   █ ▄▄▄▄▄ ┃
// ┃ █      ██████ █  ▀█▄       █ ██████      █      ███▌▐███ ███████▄ █       ┃
// ┣━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┫
// ┃ Copyright (c) 2017, the Perspective Authors.                              ┃
// ┃ ╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌ ┃
// ┃ This file is part of the Perspective library, distributed under the terms ┃
// ┃ of the [Apache License 2.0](https://www.apache.org/licenses/LICENSE-2.0). ┃
// ┗━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━┛

use extend::ext;
use js_sys::{Array, ArrayBuffer, Function, JSON, Object, Reflect, Uint8Array};
use macro_rules_attribute::apply;
use perspective_client::config::*;
use perspective_client::{
    ColumnType, TableData, TableReadFormat, UpdateData, UpdateOptions, assert_table_api,
};
use wasm_bindgen::convert::TryFromJsValue;
use wasm_bindgen::prelude::*;
use wasm_bindgen_derive::TryFromJsValue;
use wasm_bindgen_futures::spawn_local;

use crate::client::Client;
use crate::utils::{
    ApiError, ApiFuture, ApiResult, JsValueSerdeExt, LocalPollLoop, ToApiError, inherit_docs,
};
pub use crate::view::*;

#[ext]
impl Vec<(String, ColumnType)> {
    fn from_js_value(value: &JsValue) -> ApiResult<Vec<(String, ColumnType)>> {
        Ok(Object::keys(value.unchecked_ref())
            .iter()
            .map(|x| -> Result<_, JsValue> {
                let key = x.as_string().into_apierror()?;
                let val = Reflect::get(value, &x)?
                    .as_string()
                    .into_apierror()?
                    .into_serde_ext()?;

                Ok((key, val))
            })
            .collect::<Result<Vec<_>, _>>()?)
    }
}

#[ext]
pub(crate) impl TableData {
    fn from_js_value(value: &JsValue, format: Option<TableReadFormat>) -> ApiResult<TableData> {
        let err_fn = || JsValue::from(format!("Failed to construct Table {:?}", value));
        if let Some(result) = UpdateData::from_js_value_partial(value, format)? {
            Ok(result.into())
        } else if value.is_instance_of::<Object>() && Reflect::has(value, &"__get_model".into())? {
            let val = Reflect::get(value, &"__get_model".into())?
                .dyn_into::<Function>()?
                .call0(value)?;

            let view = View::try_from_js_value(val)?;
            Ok(TableData::View(view.0))
        } else if value.is_instance_of::<Object>() {
            let all_strings = || {
                Object::values(value.unchecked_ref())
                    .to_vec()
                    .iter()
                    .all(|x| x.is_string())
            };

            let all_arrays = || {
                Object::values(value.unchecked_ref())
                    .to_vec()
                    .iter()
                    .all(|x| x.is_instance_of::<Array>())
            };

            if all_strings() {
                Ok(TableData::Schema(Vec::from_js_value(value)?))
            } else if all_arrays() {
                let json = JSON::stringify(value)?.as_string().into_apierror()?;
                Ok(UpdateData::JsonColumns(json).into())
            } else {
                Err(err_fn().into())
            }
        } else {
            Err(err_fn().into())
        }
    }
}

#[ext]
pub(crate) impl UpdateData {
    fn from_js_value_partial(
        value: &JsValue,
        format: Option<TableReadFormat>,
    ) -> ApiResult<Option<UpdateData>> {
        let err_fn = || JsValue::from(format!("Failed to construct Table {:?}", value));
        if value.is_undefined() {
            Err(err_fn().into())
        } else if value.is_string() {
            match format {
                None | Some(TableReadFormat::Csv) => {
                    Ok(Some(UpdateData::Csv(value.as_string().into_apierror()?)))
                },
                Some(TableReadFormat::JsonString) => Ok(Some(UpdateData::JsonRows(
                    value.as_string().into_apierror()?,
                ))),
                Some(TableReadFormat::ColumnsString) => Ok(Some(UpdateData::JsonColumns(
                    value.as_string().into_apierror()?,
                ))),
                Some(TableReadFormat::Arrow) => Ok(Some(UpdateData::Arrow(
                    value.as_string().into_apierror()?.into_bytes().into(),
                ))),
                Some(TableReadFormat::Ndjson) => {
                    Ok(Some(UpdateData::Ndjson(value.as_string().into_apierror()?)))
                },
            }
        } else if value.is_instance_of::<ArrayBuffer>() {
            let uint8array = Uint8Array::new(value);
            let slice = uint8array.to_vec();
            match format {
                Some(TableReadFormat::Csv) => Ok(Some(UpdateData::Csv(String::from_utf8(slice)?))),
                Some(TableReadFormat::JsonString) => {
                    Ok(Some(UpdateData::JsonRows(String::from_utf8(slice)?)))
                },
                Some(TableReadFormat::ColumnsString) => {
                    Ok(Some(UpdateData::JsonColumns(String::from_utf8(slice)?)))
                },
                Some(TableReadFormat::Ndjson) => {
                    Ok(Some(UpdateData::Ndjson(String::from_utf8(slice)?)))
                },
                None | Some(TableReadFormat::Arrow) => Ok(Some(UpdateData::Arrow(slice.into()))),
            }
        } else if let Some(uint8array) = value.dyn_ref::<Uint8Array>() {
            let slice = uint8array.to_vec();
            match format {
                Some(TableReadFormat::Csv) => Ok(Some(UpdateData::Csv(String::from_utf8(slice)?))),
                Some(TableReadFormat::JsonString) => {
                    Ok(Some(UpdateData::JsonRows(String::from_utf8(slice)?)))
                },
                Some(TableReadFormat::ColumnsString) => {
                    Ok(Some(UpdateData::JsonColumns(String::from_utf8(slice)?)))
                },
                Some(TableReadFormat::Ndjson) => {
                    Ok(Some(UpdateData::Ndjson(String::from_utf8(slice)?)))
                },
                None | Some(TableReadFormat::Arrow) => Ok(Some(UpdateData::Arrow(slice.into()))),
            }
        } else if value.is_instance_of::<Array>() {
            let rows = JSON::stringify(value)?.as_string().into_apierror()?;
            Ok(Some(UpdateData::JsonRows(rows)))
        } else {
            Ok(None)
        }
    }

    fn from_js_value(value: &JsValue, format: Option<TableReadFormat>) -> ApiResult<UpdateData> {
        match TableData::from_js_value(value, format)? {
            TableData::Schema(_) => Err(ApiError::new(
                "Method cannot be called with `Schema` argument",
            )),
            TableData::Update(x) => Ok(x),
            TableData::View(_) => Err(ApiError::new(
                "Method cannot be called with `Schema` argument",
            )),
        }
    }
}

#[derive(TryFromJsValue, Clone)]
#[wasm_bindgen]
pub struct Table(pub(crate) perspective_client::Table);

assert_table_api!(Table);

impl From<perspective_client::Table> for Table {
    fn from(value: perspective_client::Table) -> Self {
        Table(value)
    }
}

impl Table {
    pub fn get_table(&self) -> &'_ perspective_client::Table {
        &self.0
    }
}

#[wasm_bindgen]
extern "C" {
    // TODO Fix me
    #[wasm_bindgen(typescript_type = "\
        string | ArrayBuffer | Record<string, unknown[]> | Record<string, unknown>[]")]
    pub type JsTableInitData;

    #[wasm_bindgen(typescript_type = "ViewConfigUpdate")]
    pub type JsViewConfig;

    #[wasm_bindgen(typescript_type = "UpdateOptions")]
    pub type JsUpdateOptions;
}

#[wasm_bindgen]
impl Table {
    #[apply(inherit_docs)]
    #[inherit_doc = "table/get_index.md"]
    #[wasm_bindgen]
    pub async fn get_index(&self) -> Option<String> {
        self.0.get_index()
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/get_client.md"]
    #[wasm_bindgen]
    pub async fn get_client(&self) -> Client {
        Client {
            close: None,
            client: self.0.get_client(),
        }
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/get_name.md"]
    #[wasm_bindgen]
    pub async fn get_name(&self) -> String {
        self.0.get_name().to_owned()
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/get_limit.md"]
    #[wasm_bindgen]
    pub async fn get_limit(&self) -> Option<u32> {
        self.0.get_limit()
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/clear.md"]
    #[wasm_bindgen]
    pub async fn clear(&self) -> ApiResult<()> {
        self.0.clear().await?;
        Ok(())
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/delete.md"]
    #[wasm_bindgen]
    pub async fn delete(&self) -> ApiResult<()> {
        self.0.delete().await?;
        Ok(())
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/size.md"]
    #[wasm_bindgen]
    pub async fn size(&self) -> ApiResult<f64> {
        Ok(self.0.size().await? as f64)
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/schema.md"]
    #[wasm_bindgen]
    pub async fn schema(&self) -> ApiResult<JsValue> {
        let schema = self.0.schema().await?;
        Ok(JsValue::from_serde_ext(&schema)?)
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/columns.md"]
    #[wasm_bindgen]
    pub async fn columns(&self) -> ApiResult<JsValue> {
        let columns = self.0.columns().await?;
        Ok(JsValue::from_serde_ext(&columns)?)
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/make_port.md"]
    #[wasm_bindgen]
    pub async fn make_port(&self) -> ApiResult<i32> {
        Ok(self.0.make_port().await?)
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/on_delete.md"]
    #[wasm_bindgen]
    pub async fn on_delete(&self, on_delete: Function) -> ApiResult<u32> {
        let emit = LocalPollLoop::new(move |()| on_delete.call0(&JsValue::UNDEFINED));
        let on_delete = Box::new(move || spawn_local(emit.poll(())));
        Ok(self.0.on_delete(on_delete).await?)
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/remove_delete.md"]
    #[wasm_bindgen]
    pub fn remove_delete(&self, callback_id: u32) -> ApiFuture<()> {
        let client = self.0.clone();
        ApiFuture::new(async move {
            client.remove_delete(callback_id).await?;
            Ok(())
        })
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/replace.md"]
    #[wasm_bindgen]
    pub async fn remove(&self, value: &JsValue, options: Option<JsUpdateOptions>) -> ApiResult<()> {
        let options = options
            .into_serde_ext::<Option<UpdateOptions>>()?
            .unwrap_or_default();

        let input = UpdateData::from_js_value(value, options.format)?;
        self.0.remove(input).await?;
        Ok(())
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/replace.md"]
    #[wasm_bindgen]
    pub async fn replace(
        &self,
        input: &JsValue,
        options: Option<JsUpdateOptions>,
    ) -> ApiResult<()> {
        let options = options
            .into_serde_ext::<Option<UpdateOptions>>()?
            .unwrap_or_default();

        let input = UpdateData::from_js_value(input, options.format)?;
        self.0.replace(input).await?;
        Ok(())
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/update.md"]
    #[wasm_bindgen]
    pub async fn update(
        &self,
        input: &JsTableInitData,
        options: Option<JsUpdateOptions>,
    ) -> ApiResult<()> {
        let options = options
            .into_serde_ext::<Option<UpdateOptions>>()?
            .unwrap_or_default();

        let input = UpdateData::from_js_value(input, options.format)?;
        self.0.update(input, options).await?;
        Ok(())
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/view.md"]
    #[wasm_bindgen]
    pub async fn view(&self, config: Option<JsViewConfig>) -> ApiResult<View> {
        let config = config
            .map(|config| js_sys::JSON::stringify(&config))
            .transpose()?
            .and_then(|x| x.as_string())
            .map(|x| serde_json::from_str(x.as_str()))
            .transpose()?;

        let view = self.0.view(config).await?;
        Ok(View(view))
    }

    #[apply(inherit_docs)]
    #[inherit_doc = "table/validate_expressions.md"]
    #[wasm_bindgen]
    pub async fn validate_expressions(&self, exprs: &JsValue) -> ApiResult<JsValue> {
        let exprs = JsValue::into_serde_ext::<Expressions>(exprs.clone())?;
        let columns = self.0.validate_expressions(exprs).await?;
        Ok(JsValue::from_serde_ext(&columns)?)
    }
}
