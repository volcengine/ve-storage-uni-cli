//! 复用用户鉴权、重试和请求追踪调用 V2 文件 Chunk API。
use super::*;
use crate::domain::chunk::{instance_endpoint, ChunkOutput, ReadChunkInput, SearchChunkInput};

impl Client {
    pub async fn search_file_chunks(
        &self,
        instance: &str,
        input: &SearchChunkInput,
    ) -> Result<ChunkOutput> {
        input.validate()?;
        let client = self.chunk_client(instance)?;
        client
            .do_idempotent_json(Method::POST, "/v2/file/search_chunk", None, Some(input))
            .await
    }

    pub async fn read_file_chunks(
        &self,
        instance: &str,
        input: &ReadChunkInput,
    ) -> Result<ChunkOutput> {
        input.validate()?;
        let client = self.chunk_client(instance)?;
        client
            .do_idempotent_json(Method::POST, "/v2/file/read_chunk", None, Some(input))
            .await
    }

    fn chunk_client(&self, instance: &str) -> Result<Self> {
        if !self.uses_oauth() {
            return Err(Error::client(
                "chunk commands require --auth-mode oauth and a user with file:read permission",
            ));
        }
        Ok(Self {
            inner: Arc::new(ClientInner {
                endpoint: instance_endpoint(&self.inner.endpoint, instance)?,
                oauth_instance_scope: Some(instance.to_string()),
                ..(*self.inner).clone()
            }),
        })
    }
}
