let (output_size, output_scale, output_transform) = (&self.output_mode_source)
            .try_into()
            .map_err(OutputDamageTrackerError::OutputNoMode)?;
        tracing::info!("render debug: output_transform = ({:.2},{:.2})", output_transform.x, output_transform.y);