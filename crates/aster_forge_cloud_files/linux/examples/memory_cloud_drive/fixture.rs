struct MemoryCloud {
    capabilities: CloudFilesCapabilities,
    scope: CloudScope,
    items: HashMap<CloudItemKey, CloudItem>,
    children: HashMap<CloudItemKey, Vec<CloudItemKey>>,
    contents: HashMap<CloudItemKey, (ContentRevision, Bytes)>,
    page_size: usize,
}

impl MemoryCloud {
    #[expect(
        clippy::too_many_lines,
        reason = "the example fixture defines one coherent paged namespace with nested files, content, capabilities, and stable inodes"
    )]
    fn fixture() -> ExampleResult<(Self, Vec<LinuxInodeRecord>)> {
        let scope = CloudScope::new(
            CloudNamespaceId::new("aster-forge-memory-example")?,
            CloudRootId::new("linux-default")?,
        );
        let root_key = key(&scope, "root")?;
        let docs_key = key(&scope, "docs")?;
        let hello_key = key(&scope, "hello")?;
        let readme_key = key(&scope, "readme")?;
        let numbers_key = key(&scope, "numbers")?;
        let guide_key = key(&scope, "guide")?;

        let root = CloudItem::directory(
            root_key.clone(),
            None,
            "AsterForge Memory Cloud",
            metadata_revision("root-m1")?,
        )?;
        let docs = CloudItem::directory(
            docs_key.clone(),
            Some(root_key.item_id().clone()),
            "docs",
            metadata_revision("docs-m1")?,
        )?;
        let hello = file(
            hello_key.clone(),
            &root_key,
            "hello.txt",
            "hello-c1",
            b"Hello from the AsterForge Linux FUSE memory cloud.\n",
        )?;
        let readme = file(
            readme_key.clone(),
            &root_key,
            "readme.txt",
            "readme-c1",
            b"This writable mount uses product-neutral synthetic dirty staging.\n",
        )?;
        let numbers_bytes = (0_u32..4096).flat_map(u32::to_le_bytes).collect::<Vec<_>>();
        let numbers = file(
            numbers_key.clone(),
            &root_key,
            "numbers.bin",
            "numbers-c1",
            &numbers_bytes,
        )?;
        let guide = file(
            guide_key.clone(),
            &docs_key,
            "guide.txt",
            "guide-c1",
            b"Nested directory enumeration and content fetch are active.\n",
        )?;

        let items = HashMap::from([
            (root_key.clone(), root.clone()),
            (docs_key.clone(), docs.clone()),
            (hello_key.clone(), hello.clone()),
            (readme_key.clone(), readme.clone()),
            (numbers_key.clone(), numbers.clone()),
            (guide_key.clone(), guide.clone()),
        ]);
        let children = HashMap::from([
            (
                root_key.clone(),
                vec![
                    docs_key.clone(),
                    hello_key.clone(),
                    readme_key.clone(),
                    numbers_key.clone(),
                ],
            ),
            (docs_key.clone(), vec![guide_key.clone()]),
        ]);
        let contents = [
            (
                &hello,
                Bytes::from_static(b"Hello from the AsterForge Linux FUSE memory cloud.\n"),
            ),
            (
                &readme,
                Bytes::from_static(
                    b"This writable mount uses product-neutral synthetic dirty staging.\n",
                ),
            ),
            (&numbers, Bytes::from(numbers_bytes)),
            (
                &guide,
                Bytes::from_static(b"Nested directory enumeration and content fetch are active.\n"),
            ),
        ]
        .into_iter()
        .map(|(item, bytes)| {
            let revision = item
                .content()
                .map(|content| content.revision().clone())
                .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
            Ok((item.key().clone(), (revision, bytes)))
        })
        .collect::<BackendResult<HashMap<_, _>>>()?;

        let capabilities = CloudFilesCapabilities {
            identity: IdentityCapabilities {
                stable_item_ids: true,
                path_independent_item_ids: true,
                same_root_move_preserves_identity: true,
                cross_root_move_preserves_identity: false,
            },
            revisions: RevisionCapabilities {
                separate_metadata_and_content: true,
                conditional_metadata_mutation: false,
                conditional_content_access: true,
            },
            enumeration: EnumerationCapabilities {
                paged: true,
                stable_snapshot: true,
            },
            hydration: HydrationCapabilities {
                whole_file: true,
                range: Some(RangeHydrationCapabilities {
                    alignment: Alignment::ONE,
                    partial_cancel: false,
                }),
                ..HydrationCapabilities::default()
            },
            mutations: MutationCapabilities::default(),
            ..CloudFilesCapabilities::default()
        };

        let generation = LinuxInodeGeneration::new(1)?;
        let records = [root, docs, hello, readme, numbers, guide]
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let inode = u64::try_from(index + 1)
                    .map_err(|error| Box::new(error) as Box<dyn Error + Send + Sync>)?;
                Ok(LinuxInodeRecord::new(
                    item.key().clone(),
                    LinuxInode::new(inode)?,
                    generation,
                ))
            })
            .collect::<ExampleResult<Vec<_>>>()?;

        Ok((
            Self {
                capabilities,
                scope,
                items,
                children,
                contents,
                page_size: 2,
            },
            records,
        ))
    }

    fn encode_cursor(parent: &CloudItemKey, offset: usize) -> BackendResult<PageCursor> {
        PageCursor::new(format!("{}:{offset}", parent.item_id()).into_bytes())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
    }

    fn decode_cursor(parent: &CloudItemKey, cursor: &PageCursor) -> BackendResult<usize> {
        let value = std::str::from_utf8(cursor.as_bytes())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
        let prefix = format!("{}:", parent.item_id());
        value
            .strip_prefix(&prefix)
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?
            .parse()
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))
    }
}

#[async_trait]
impl CloudMetadataBackend for MemoryCloud {
    async fn get_item(&self, key: &CloudItemKey) -> BackendResult<CloudItem> {
        self.items
            .get(key)
            .cloned()
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))
    }

    async fn list_children(
        &self,
        parent: &CloudItemKey,
        cursor: Option<&PageCursor>,
    ) -> BackendResult<CloudItemPage> {
        let parent_item = self
            .items
            .get(parent)
            .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::NotFound))?;
        if parent_item.kind() != CloudItemKind::Directory {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::InvalidRequest,
            ));
        }
        let keys: &[CloudItemKey] = self.children.get(parent).map_or(&[], Vec::as_slice);
        let offset = match cursor {
            Some(cursor) => Self::decode_cursor(parent, cursor)?,
            None => 0,
        };
        if offset > keys.len() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::InvalidRequest,
            ));
        }
        let end = offset.saturating_add(self.page_size).min(keys.len());
        let items = keys[offset..end]
            .iter()
            .map(|key| {
                self.items
                    .get(key)
                    .cloned()
                    .ok_or_else(|| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))
            })
            .collect::<BackendResult<Vec<_>>>()?;
        let next = if end < keys.len() {
            Some(Self::encode_cursor(parent, end)?)
        } else {
            None
        };
        Ok(CloudItemPage::new(items, next))
    }

    async fn changes_since(
        &self,
        scope: &CloudScope,
        _cursor: Option<&ChangeCursor>,
    ) -> BackendResult<ChangePage> {
        if scope != &self.scope {
            return Err(CloudBackendError::new(CloudBackendErrorKind::NotFound));
        }
        Err(CloudBackendError::new(CloudBackendErrorKind::Unsupported))
    }
}

#[async_trait]
impl CloudContentBackend for MemoryCloud {
    async fn read_content(
        &self,
        request: &ContentReadRequest,
    ) -> BackendResult<ContentReadResponse> {
        let Some((revision, content)) = self.contents.get(request.key()) else {
            return Err(CloudBackendError::new(CloudBackendErrorKind::NotFound));
        };
        if revision != request.revision() {
            return Err(CloudBackendError::new(
                CloudBackendErrorKind::PreconditionFailed,
            ));
        }
        let total_size = u64::try_from(content.len())
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        let (offset, bytes) = match request.read_range() {
            ContentReadRange::Whole => (0, content.clone()),
            ContentReadRange::Range(range) => {
                if range.offset() > total_size {
                    return Err(CloudBackendError::new(
                        CloudBackendErrorKind::InvalidRequest,
                    ));
                }
                let start = usize::try_from(range.offset())
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
                let end = usize::try_from(range.end_exclusive().min(total_size))
                    .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidRequest))?;
                (range.offset(), content.slice(start..end))
            }
        };
        let response = ContentReadResponse::new(revision.clone(), offset, bytes, total_size)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        request
            .validate_response(&response)
            .map_err(|_| CloudBackendError::new(CloudBackendErrorKind::InvalidResponse))?;
        Ok(response)
    }
}

impl CloudFilesBackend for MemoryCloud {
    fn capabilities(&self) -> CloudFilesCapabilities {
        self.capabilities
    }
}
