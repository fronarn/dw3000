use core::num::Wrapping;

use embedded_hal::{blocking::spi, digital::v2::OutputPin};

use crate::{ll, Config, Error, Ready, Uninitialized, DW3000};
//use rtt_target::{rprintln};

impl<SPI, CS> DW3000<SPI, CS, Uninitialized>
where
	SPI: spi::Transfer<u8> + spi::Write<u8>,
	CS: OutputPin,
{
	/// Create a new instance of `DW3000`
	///
	/// Requires the SPI peripheral and the chip select pin that are connected
	/// to the DW3000.
	pub fn new(spi: SPI, chip_select: CS) -> Self {
		DW3000 {
			ll:    ll::DW3000::new(spi, chip_select),
			seq:   Wrapping(0),
			state: Uninitialized,
		}
	}

	/// Initialize the DW3000
	/// Basicaly, this is the pll configuration. We want to have a locked pll in order to provide a constant speed clock.
	/// This is important when using th clock to measure distances.
	/// At the end of this function, pll is locked and it can be checked by the bit CPLOCK in SYS_STATUS register (see state_test example)
	pub fn init(mut self) -> Result<DW3000<SPI, CS, Uninitialized>, Error<SPI, CS>> {
		// Wait for the IDLE_RC state
		while self.ll.sys_status().read()?.rcinit() == 0 {}
		// need to change default cal value for pll (page164)
		self.ll.pll_cal().modify(|_, w| w.pll_cfg_ld(0x81))?;
		// clear cplock
		self.ll.sys_status().write(|w| w.cplock(0))?;
		// select PLL mode auto
		self.ll.clk_ctrl().modify(|_, w| w.sys_clk(0))?;
		// set ainit2idle
		self.ll.seq_ctrl().modify(|_, w| w.ainit2idle(1))?;
		// Set the on wake up switch from idle RC to idle PLL
		self.ll.aon_dig_cfg().modify(|_, w| w.onw_go2idle(1))?;
		// wait for CPLOCK to be set
		while self.ll.sys_status().read()?.cplock() == 0 {}

		Ok(DW3000 {
			ll:    self.ll,
			seq:   self.seq,
			state: Uninitialized,
		})
	}

	/// Configuration of the DW3000, need to be called after an init.
	/// This function need to be improved. TODO
	/// There is several steps to do on this function that improve the sending and reception of a message.
	/// Without doing this, the receiver almost never receive a frame form transmitter 
	/// FIRST STEP : configuration depending on CONFIG chosen. Lot of register all around the datasheet can be changed in order to improve the signal
	/// Some register needs to be changed without a lot of explanation so we tried to gather all of them in this function 
	pub fn config(mut self, config: Config) -> Result<DW3000<SPI, CS, Ready>, Error<SPI, CS>> {
		
		// CONFIGURATION DEPENDING ON PRF AND CHANNEL
		// Register DGC_CFG (page 124)
		self.ll.dgc_lut_0().modify(|_, w| w.value(config.channel.get_recommended_dgc_lut_0()))?;
		self.ll.dgc_lut_1().modify(|_, w| w.value(config.channel.get_recommended_dgc_lut_1()))?;
		self.ll.dgc_lut_2().modify(|_, w| w.value(config.channel.get_recommended_dgc_lut_2()))?;
		self.ll.dgc_lut_3().modify(|_, w| w.value(config.channel.get_recommended_dgc_lut_3()))?;
		self.ll.dgc_lut_4().modify(|_, w| w.value(config.channel.get_recommended_dgc_lut_4()))?;
		self.ll.dgc_lut_5().modify(|_, w| w.value(config.channel.get_recommended_dgc_lut_5()))?;
		self.ll.dgc_lut_6().modify(|_, w| w.value(config.channel.get_recommended_dgc_lut_6()))?;
		self.ll.dgc_cfg0().modify(|_, w| w.value(0x10000240))?;
		self.ll.dgc_cfg1().modify(|_, w| w.value(0x1b6da489))?;
		// page 126
		self.ll.dgc_cfg().modify(|_, w| {
			w.rx_tune_en(
				config
					.pulse_repetition_frequency
					.get_recommended_rx_tune_en(),
			)
			.thr_64(0x32)
		})?;

		// Register CHAN_CTRL (page 110) general configuration for channel control
		// used to select transmit and receive channels, and configure preamble codes and 
		// some related parameters.
		self.ll.chan_ctrl().modify(|_, w| {
			w
				.rf_chan(config.channel as u8) // 0 if channel5 and 1 if channel9
				.sfd_type(config.sfd_sequence as u8)
				.tx_pcode( // set the PRF for transmitter
					config
						.channel
						.get_recommended_preamble_code(config.pulse_repetition_frequency),
				)
				.rx_pcode( // set the PRF for receiver
					config
						.channel
						.get_recommended_preamble_code(config.pulse_repetition_frequency),
				)
		})?;
		self.ll.rf_tx_ctrl_1().modify(|_, w| w.value(0x0E))?;
		self.ll
				.rf_tx_ctrl_2()
				.modify(|_, w| w.value(config.channel.get_recommended_rf_tx_ctrl_2()))?;
		self.ll
				.pll_cfg()
				.modify(|_, w| w.value(config.channel.get_recommended_pll_conf()))?;

		// TRANSMITTER (TX_FCTRL) CONFIG (page 85) define BITRATE
		// DEFINED IN SEND FUNCTION (READY STATE)

		// RECEIVER (DRX_CONF) CONF
		self.ll.dtune0().modify(|_, w| {
			w.pac(config.preamble_length.get_recommended_pac_size())
				.dt0b4(0)
		})?;
		self.ll.dtune3().modify(|_, w| w.value(0xAF5F35CC))?;

		// page 155
		self.ll.ldo_rload().write(|w| w.value(0x14))?;
		// page 164
		self.ll.pll_cal().write(|w| w.pll_cfg_ld(0x1))?;


		//  FRAME FILTERING CONFIGURATION
		if config.frame_filtering {
			self.ll.sys_cfg().modify(
				|_, w| w.ffen(0b1), // enable frame filtering
			)?;
			self.ll.ff_cfg().modify(
				|_, w| {
					w.ffab(0b1) // receive beacon frames
						.ffad(0b1) // receive data frames
						.ffaa(0b1) // receive acknowledgement frames
						.ffam(0b1) // Allow MAC command frame reception
						// NEED ADD MORE
				},
			)?;
		}
		else {
			self.ll.sys_cfg().modify(|_, w| w.ffen(0b0))?; // disable frame filtering
		}
	
		// CALIBRATION 
		// RF_CONF
		let val = self.ll.ldo_ctrl().read()?.value();
		self.ll.ldo_ctrl().modify(|_, w| w.value(val | 0x105))?;

		self.ll.rx_cal().modify(|_, w| {
			w
				.comp_dly(0x2)
				.cal_mode(1)
		})?;
		self.ll.rx_cal().modify(|_, w| w.cal_en(1))?;
		while self.ll.rx_cal_sts().read()?.value() == 0 {}
		self.ll.rx_cal().modify(|_, w| {
			w
				.cal_mode(0)
				.cal_en(0)
		})?;
		self.ll.rx_cal_sts().write(|w| w.value(1))?;

		if self.ll.rx_cal_resi().read()?.value() == 0x1fffffff || self.ll.rx_cal_resq().read()?.value() == 0x1fffffff {
			return Err(Error::InvalidConfiguration)
		}
		self.ll.ldo_ctrl().write(|w| w.value(val))?;

		Ok(DW3000 {
			ll:    self.ll,
			seq:   self.seq,
			state: Ready,
		})
	}
}
